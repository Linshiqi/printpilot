//! 看图建模流水线(docs/adr/0003-code-cad-build123d.md):
//!
//!   参考图 ─视觉模型→ 设计规格 ─(人审)→ 代码模型写 build123d 脚本 → 真引擎执行 → 自动检查
//!        ↑                                              └── 不通过:把原因喂回去修(有次数上限)──┘
//!        └──────────── 看图复核:渲染图 vs 参考图 → 差异清单 → 逐条走「指令修补」 ────────────┘
//!
//! 质量靠的不是「模型一次写对」,而是这几条:
//! - 规格先给人看、给人改,再去写代码——尺寸和特征清单是人确认过的;
//! - 每一版代码都在真的 OpenCascade 上跑,报错(带行号)原样喂回去修;
//! - 跑通不等于合格:必须恰好 1 个实体、B-rep 有效、贴床、尺寸和规格对得上、放得进成型空间;
//! - 修复次数用完了,交付「问题最少的那一版」并如实列出遗留问题,而不是假装成功或者整个失败;
//! - 局部修改只动相关的段,改了哪些段由我们自己比对出来(不信模型的自述)。

use std::time::Instant;

use async_trait::async_trait;
use pp_cad::contract::{changed_sections, contract_issues, extract_code, sections};
use pp_cad::{parse_params, CadError};
use pp_common::cad::{CadBuildReport, CadMetrics, CadPick, CadProblem, CadReview, DesignSpec};
use pp_providers::llm::{ChatMessage, ChatRequest, ChatResponse, LlmPricing, LlmProvider, Usage};
use pp_providers::ProviderError;

use crate::json::ask_json;
use crate::AgentError;

const SPEC_PROMPT: &str = include_str!("../prompts/cad_spec.md");
pub(crate) const CODE_PROMPT: &str = include_str!("../prompts/cad_code.md");
const EDIT_PROMPT: &str = include_str!("../prompts/cad_edit.md");
const REVIEW_PROMPT: &str = include_str!("../prompts/cad_review.md");

/// 自动加的「贴床」行。用包围盒现算,所以改了参数之后仍然成立。
const PLATE_SNAP_LINE: &str = "result = Pos(0, 0, -result.bounding_box().min.Z) * result  # 贴到打印床 (z = 0)";

#[derive(Debug, Clone, PartialEq)]
pub struct CadConfig {
    /// 看图的模型。DeepSeek 目前只有 flash 支持视觉
    pub vision_model: String,
    pub vision_pricing: LlmPricing,
    pub vision_thinking: bool,
    /// 写代码的模型:用最强的那个,开思考。它看不见图,只看规格
    pub code_model: String,
    pub code_pricing: LlmPricing,
    pub code_thinking: bool,
    /// 思考模式下思维链也算在 max_tokens 里,所以要给足
    pub code_max_tokens: u32,
    /// 第一版之后最多再修几轮
    pub max_repairs: u32,
    pub build_mm: [f64; 3],
}

impl Default for CadConfig {
    fn default() -> Self {
        Self {
            vision_model: "deepseek-flash".into(),
            vision_pricing: LlmPricing::deepseek_flash(),
            vision_thinking: true,
            code_model: "deepseek-v4-pro".into(),
            code_pricing: LlmPricing::deepseek_pro(),
            code_thinking: true,
            code_max_tokens: 16_000,
            max_repairs: 3,
            build_mm: [256.0, 256.0, 256.0],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CadProgress {
    ReadingImage,
    /// 第几版(从 1 起)
    WritingCode { attempt: u32 },
    Running { attempt: u32 },
    /// 这一版有几条问题,准备修
    Repairing { attempt: u32, problems: u32 },
    Reviewing,
    Done,
}

/// 「执行一段建模代码」的能力。真实实现起 Python 子进程(src-tauri),测试里用假的。
#[async_trait]
pub trait CadExecutor: Send + Sync {
    async fn execute(&self, code: &str) -> Result<CadMetrics, CadError>;
}

/// 生成 / 修补的产出。
#[derive(Debug, Clone, PartialEq)]
pub struct CadBuild {
    /// 交付的代码;一版能跑的都没有时,是最后一版代码(用户可以接着手改)
    pub code: String,
    /// `None` = 修复次数用完,没有任何一版能跑
    pub metrics: Option<CadMetrics>,
    pub report: CadBuildReport,
}

// ---------------------------------------------------------------- 检查

fn close(got: f64, want: f64) -> bool {
    (got - want).abs() <= (want.abs() * 0.05).max(1.0)
}

/// 包围盒和规格对得上吗。高度(Z)必须对上;X / Y 允许互换——零件在床上转 90° 不算错。
fn size_matches(got: [f64; 3], want: [f64; 3]) -> bool {
    close(got[2], want[2])
        && ((close(got[0], want[0]) && close(got[1], want[1])) || (close(got[0], want[1]) && close(got[1], want[0])))
}

/// 能放进成型空间吗(同样允许 X / Y 互换)。
fn fits_build(size: [f64; 3], build: [f64; 3]) -> bool {
    size[2] <= build[2] && ((size[0] <= build[0] && size[1] <= build[1]) || (size[0] <= build[1] && size[1] <= build[0]))
}

/// 「跑通了」之后的检查:代码契约 + 几何 + 参数。空 = 合格。
pub fn check_build(code: &str, metrics: &CadMetrics, spec: Option<&DesignSpec>, build_mm: [f64; 3]) -> Vec<CadProblem> {
    let mut problems: Vec<CadProblem> = contract_issues(code).into_iter().map(|detail| CadProblem::Contract { detail }).collect();

    if metrics.solids != 1 {
        problems.push(CadProblem::Solids { count: metrics.solids });
    }
    if !metrics.is_valid {
        problems.push(CadProblem::Invalid);
    }
    if metrics.bbox_min[2].abs() > 0.01 {
        problems.push(CadProblem::OffPlate { z: metrics.bbox_min[2] });
    }
    if let Some(spec) = spec {
        if spec.overall_mm.iter().all(|v| *v > 0.0) && !size_matches(metrics.size, spec.overall_mm) {
            problems.push(CadProblem::Size {
                got: metrics.size,
                want: spec.overall_mm,
            });
        }
    }
    if !fits_build(metrics.size, build_mm) {
        problems.push(CadProblem::TooBig {
            size: metrics.size,
            build: build_mm,
        });
    }

    let params = parse_params(code);
    if params.is_empty() {
        problems.push(CadProblem::NoParams);
    }
    for p in params {
        let below = p.min.is_some_and(|min| p.value < min);
        let above = p.max.is_some_and(|max| p.value > max);
        if below || above {
            problems.push(CadProblem::ParamRange {
                name: p.name,
                value: p.value,
                min: p.min,
                max: p.max,
            });
        }
    }
    problems
}

fn feedback(problems: &[CadProblem]) -> String {
    let mut out = String::from("The script did not pass. Problems:\n");
    for (i, p) in problems.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, p.for_model()));
    }
    out.push_str(
        "\nFix these problems and output the COMPLETE corrected script in a single ```python fence. \
Keep everything that already works unchanged.",
    );
    out
}

fn same_code(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    norm(a) == norm(b)
}

// ---------------------------------------------------------------- 写代码 → 执行 → 修

struct Candidate {
    attempt: u32,
    code: String,
    metrics: CadMetrics,
    problems: Vec<CadProblem>,
    snapped: bool,
}

struct Evaluated {
    code: String,
    metrics: Option<CadMetrics>,
    problems: Vec<CadProblem>,
    snapped: bool,
}

/// 执行一版代码并检查。`Err` 只在引擎本身出问题时返回(没装、起不来)——那不是模型能修的。
async fn evaluate(
    exec: &dyn CadExecutor,
    code: String,
    spec: Option<&DesignSpec>,
    build_mm: [f64; 3],
    runs: &mut u32,
) -> Result<Evaluated, AgentError> {
    *runs += 1;
    let metrics = match exec.execute(&code).await {
        Ok(m) => m,
        Err(CadError::Script(error)) => {
            // 契约问题和运行错误一起报,少来回一轮
            let mut problems: Vec<CadProblem> =
                contract_issues(&code).into_iter().map(|detail| CadProblem::Contract { detail }).collect();
            problems.push(CadProblem::Script { error });
            return Ok(Evaluated {
                code,
                metrics: None,
                problems,
                snapped: false,
            });
        }
        Err(CadError::Timeout(d)) => {
            return Ok(Evaluated {
                code,
                metrics: None,
                problems: vec![CadProblem::Timeout { secs: d.as_secs() }],
                snapped: false,
            })
        }
        Err(e) => return Err(AgentError::Engine(e)),
    };

    let problems = check_build(&code, &metrics, spec, build_mm);
    // 唯一的问题是「没贴床」:自己补一行就行,不值得再花一次模型调用
    if matches!(problems.as_slice(), [CadProblem::OffPlate { .. }]) {
        let snapped = format!("{}\n{PLATE_SNAP_LINE}\n", code.trim_end());
        *runs += 1;
        if let Ok(m2) = exec.execute(&snapped).await {
            if check_build(&snapped, &m2, spec, build_mm).is_empty() {
                return Ok(Evaluated {
                    code: snapped,
                    metrics: Some(m2),
                    problems: Vec::new(),
                    snapped: true,
                });
            }
        }
    }
    Ok(Evaluated {
        code,
        metrics: Some(metrics),
        problems,
        snapped: false,
    })
}

/// 问一次;空内容之类的坏响应再问一次(DeepSeek 文档明说偶尔会这样)。
pub(crate) async fn chat_once(llm: &dyn LlmProvider, req: &ChatRequest, calls: &mut u32) -> Result<ChatResponse, ProviderError> {
    *calls += 1;
    match llm.chat(req.clone()).await {
        Err(ProviderError::BadResponse(why)) => {
            log::warn!("[cad] 坏响应,重试一次:{why}");
            *calls += 1;
            llm.chat(req.clone()).await
        }
        other => other,
    }
}

/// 调用方已经拿到手的第一版回答(对话式建模:同一次调用既判断意图又出代码,不该为了进循环再问一遍)。
pub(crate) struct FirstAnswer {
    pub response: ChatResponse,
    /// 为了拿到它调了几次模型
    pub calls: u32,
}

/// 写代码 → 执行 → 检查 → 把问题喂回去,直到合格或次数用完。`original` 有值 = 指令修补。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_loop(
    llm: &dyn LlmProvider,
    exec: &dyn CadExecutor,
    mut messages: Vec<ChatMessage>,
    spec: Option<&DesignSpec>,
    original: Option<&str>,
    mut first: Option<FirstAnswer>,
    cfg: &CadConfig,
    on_progress: &(dyn Fn(CadProgress) + Send + Sync),
) -> Result<CadBuild, AgentError> {
    let started = Instant::now();
    let mut usage = Usage::default();
    let mut report = CadBuildReport::default();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut last_code = original.unwrap_or_default().to_string();

    for attempt in 1..=cfg.max_repairs + 1 {
        let answer = match first.take() {
            Some(f) => {
                report.llm_calls += f.calls;
                f.response
            }
            None => {
                on_progress(CadProgress::WritingCode { attempt });
                let req = ChatRequest::new(&cfg.code_model, messages.clone())
                    .thinking(cfg.code_thinking)
                    .max_tokens(cfg.code_max_tokens);
                match chat_once(llm, &req, &mut report.llm_calls).await {
                    Ok(a) => a,
                    // 已经有一版能用的了:限流、断网这类中途故障不该让前面的成果作废
                    Err(e) if !candidates.is_empty() => {
                        log::warn!("[cad] 第 {attempt} 版没拿到回答({e}),交付之前的版本");
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        };
        usage.add(answer.usage);
        let code = extract_code(&answer.content);
        last_code = code.clone();

        let evaluated = if original.is_some_and(|old| same_code(old, &code)) {
            Evaluated {
                code,
                metrics: None,
                problems: vec![CadProblem::Unchanged],
                snapped: false,
            }
        } else {
            on_progress(CadProgress::Running { attempt });
            evaluate(exec, code, spec, cfg.build_mm, &mut report.runs).await?
        };

        let passed = evaluated.problems.is_empty();
        if !passed {
            report.rounds.push(evaluated.problems.clone());
        }
        if let Some(metrics) = evaluated.metrics {
            candidates.push(Candidate {
                attempt,
                code: evaluated.code,
                metrics,
                problems: evaluated.problems.clone(),
                snapped: evaluated.snapped,
            });
        }
        if passed {
            break;
        }
        if attempt <= cfg.max_repairs {
            on_progress(CadProgress::Repairing {
                attempt,
                problems: evaluated.problems.len() as u32,
            });
            messages.push(ChatMessage::assistant(answer.content));
            messages.push(ChatMessage::user(feedback(&evaluated.problems)));
        }
    }

    report.tokens_in = usage.prompt_tokens;
    report.tokens_out = usage.completion_tokens;
    report.cost_fen = cfg.code_pricing.cost_fen(usage);
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    on_progress(CadProgress::Done);

    // 问题最少的那一版;一样多就取后面的(它看过更多反馈)
    let best = candidates.into_iter().min_by_key(|c| (c.problems.len(), std::cmp::Reverse(c.attempt)));
    Ok(match best {
        Some(c) => {
            report.warnings = c.problems;
            report.plate_snapped = c.snapped;
            if let Some(old) = original {
                report.changed_sections = changed_sections(old, &c.code);
            }
            CadBuild {
                code: c.code,
                metrics: Some(c.metrics),
                report,
            }
        }
        None => CadBuild {
            code: last_code,
            metrics: None,
            report,
        },
    })
}

// ---------------------------------------------------------------- 1. 看图 → 设计规格

fn validate_spec(spec: &DesignSpec) -> Result<(), String> {
    if spec.name.trim().is_empty() {
        return Err("name 不能为空".into());
    }
    if spec.overall_mm.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(format!("overall_mm 必须是三个正数(毫米),现在是 {:?}", spec.overall_mm));
    }
    if !spec.suitable {
        if spec.unsuitable_reason.trim().is_empty() {
            return Err("suitable 为 false 时必须在 unsuitable_reason 里说明原因".into());
        }
        return Ok(());
    }
    if spec.features.is_empty() || spec.features.len() > 12 {
        return Err(format!("features 应为 1~12 个,现在是 {} 个", spec.features.len()));
    }
    for f in &spec.features {
        if f.name.trim().is_empty() || f.description.trim().is_empty() {
            return Err("每个特征都要有 name 和 description".into());
        }
        if let Some((k, v)) = f.dimensions.iter().find(|(_, v)| !v.is_finite() || **v < 0.0) {
            return Err(format!("特征「{}」的尺寸 {k} = {v} 不合理", f.name));
        }
    }
    Ok(())
}

fn vision_report(usage: Usage, calls: u32, pricing: &LlmPricing, started: Instant) -> CadBuildReport {
    CadBuildReport {
        llm_calls: calls,
        tokens_in: usage.prompt_tokens,
        tokens_out: usage.completion_tokens,
        cost_fen: pricing.cost_fen(usage),
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..Default::default()
    }
}

/// 参考图 +(可选的)文字要求 → 设计规格。`images` 是 data URL;没有图时只凭文字也能出规格。
pub async fn design_spec(
    llm: &dyn LlmProvider,
    brief: &str,
    images: &[String],
    cfg: &CadConfig,
    on_progress: &(dyn Fn(CadProgress) + Send + Sync),
) -> Result<(DesignSpec, CadBuildReport), AgentError> {
    if brief.trim().is_empty() && images.is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    on_progress(CadProgress::ReadingImage);
    let build = format!("{} × {} × {}", cfg.build_mm[0], cfg.build_mm[1], cfg.build_mm[2]);
    let system = SPEC_PROMPT.replace("{{build}}", &build);
    let text = if brief.trim().is_empty() {
        "请根据参考图给出设计规格。".to_string()
    } else {
        format!("用户的要求:\n{}", brief.trim())
    };
    let req = ChatRequest::new(
        &cfg.vision_model,
        vec![ChatMessage::system(system), ChatMessage::user_with_images(text, images.to_vec())],
    )
    .thinking(cfg.vision_thinking)
    .max_tokens(8000);
    let answer = ask_json::<DesignSpec, _>(llm, req, validate_spec).await?;
    on_progress(CadProgress::Done);
    Ok((answer.value, vision_report(answer.usage, answer.calls, &cfg.vision_pricing, started)))
}

// ---------------------------------------------------------------- 2. 规格 → 模型

fn spec_block(spec: &DesignSpec, build_mm: [f64; 3]) -> String {
    let mut out = format!(
        "## Design spec\nName: {}\nSummary: {}\nOverall bounding box (mm): X {} x Y {} x Z {}\nPrinter build volume (mm): {} x {} x {}\n\n### Features (build them in this order)\n",
        spec.name.trim(),
        spec.summary.trim(),
        spec.overall_mm[0],
        spec.overall_mm[1],
        spec.overall_mm[2],
        build_mm[0],
        build_mm[1],
        build_mm[2],
    );
    for (i, f) in spec.features.iter().enumerate() {
        out.push_str(&format!("{}. {} — {}\n", i + 1, f.name.trim(), f.description.trim()));
        if !f.dimensions.is_empty() {
            let dims: Vec<String> = f.dimensions.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            out.push_str(&format!("   {}\n", dims.join(", ")));
        }
    }
    if !spec.assumptions.is_empty() {
        out.push_str("\n### Assumptions\n");
        for a in &spec.assumptions {
            out.push_str(&format!("- {}\n", a.trim()));
        }
    }
    if !spec.print_notes.trim().is_empty() {
        out.push_str(&format!("\n### Print notes\n{}\n", spec.print_notes.trim()));
    }
    out
}

/// 按(人审过的)规格写 build123d 脚本,在真引擎上跑通并通过检查。
pub async fn generate_model(
    llm: &dyn LlmProvider,
    exec: &dyn CadExecutor,
    spec: &DesignSpec,
    cfg: &CadConfig,
    on_progress: &(dyn Fn(CadProgress) + Send + Sync),
) -> Result<CadBuild, AgentError> {
    if spec.features.is_empty() {
        return Err(AgentError::InvalidOutput("设计规格里没有任何特征".into()));
    }
    let messages = vec![ChatMessage::system(CODE_PROMPT), ChatMessage::user(spec_block(spec, cfg.build_mm))];
    build_loop(llm, exec, messages, Some(spec), None, None, cfg, on_progress).await
}

// ---------------------------------------------------------------- 3. 指令修补(局部修改)

/// 按一句话指令修改现有脚本:只动相关的段,其余逐字保留。改了哪些段写在报告的 `changed_sections` 里。
pub async fn edit_model(
    llm: &dyn LlmProvider,
    exec: &dyn CadExecutor,
    code: &str,
    instruction: &str,
    pick: Option<CadPick>,
    cfg: &CadConfig,
    on_progress: &(dyn Fn(CadProgress) + Send + Sync),
) -> Result<CadBuild, AgentError> {
    if instruction.trim().is_empty() || code.trim().is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let names: Vec<String> = sections(code).into_iter().map(|s| s.name).collect();
    let mut user = format!(
        "## Current script\n```python\n{}\n```\n\n## Sections\n{}\n\n## Requested change\n{}\n",
        code.trim_end(),
        names.join(", "),
        instruction.trim()
    );
    if let Some(p) = pick {
        user.push_str(&format!(
            "\n## Picked location\npoint = ({:.2}, {:.2}, {:.2}) mm",
            p.point[0], p.point[1], p.point[2]
        ));
        if let Some(n) = p.normal {
            user.push_str(&format!(", face normal = ({:.2}, {:.2}, {:.2})", n[0], n[1], n[2]));
        }
        user.push('\n');
    }
    let system = format!("{EDIT_PROMPT}\n---\n\n{CODE_PROMPT}");
    let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];
    build_loop(llm, exec, messages, None, Some(code), None, cfg, on_progress).await
}

// ---------------------------------------------------------------- 4. 看图复核

fn validate_review(r: &CadReview) -> Result<(), String> {
    if !r.matches && r.differences.iter().all(|d| d.trim().is_empty()) {
        return Err("matches 为 false 时 differences 不能为空".into());
    }
    if r.differences.len() > 8 {
        return Err("differences 最多 5 条,只列最重要的".into());
    }
    Ok(())
}

/// 渲染图 vs 参考图。差异写成可以直接执行的修改指令——界面上每条都能一键交给 `edit_model`。
pub async fn review_model(
    llm: &dyn LlmProvider,
    reference_images: &[String],
    render_images: &[String],
    spec: Option<&DesignSpec>,
    cfg: &CadConfig,
    on_progress: &(dyn Fn(CadProgress) + Send + Sync),
) -> Result<(CadReview, CadBuildReport), AgentError> {
    if reference_images.is_empty() || render_images.is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    on_progress(CadProgress::Reviewing);
    let mut text = format!(
        "按顺序给出 {} 张参考图,然后是 {} 张模型渲染图(不同视角)。",
        reference_images.len(),
        render_images.len()
    );
    if let Some(spec) = spec {
        text.push_str(&format!(
            "\n设计规格:{}({} × {} × {} mm)。特征:{}。",
            spec.name,
            spec.overall_mm[0],
            spec.overall_mm[1],
            spec.overall_mm[2],
            spec.features.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join("、")
        ));
    }
    let images: Vec<String> = reference_images.iter().chain(render_images).cloned().collect();
    let req = ChatRequest::new(
        &cfg.vision_model,
        vec![ChatMessage::system(REVIEW_PROMPT), ChatMessage::user_with_images(text, images)],
    )
    .thinking(cfg.vision_thinking)
    .max_tokens(4000);
    let answer = ask_json::<CadReview, _>(llm, req, validate_review).await?;
    let mut review = answer.value;
    review.differences.retain(|d| !d.trim().is_empty());
    review.differences.truncate(5);
    on_progress(CadProgress::Done);
    Ok((review, vision_report(answer.usage, answer.calls, &cfg.vision_pricing, started)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_common::cad::{CadScriptError, DesignFeature};
    use pp_providers::llm::MockLlm;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    const GOOD: &str = "from build123d import *\n\
# ---- PARAMS ----\n\
width = 60.0  # mm | 总宽 | [20, 200]\n\
height = 6.0  # mm | 厚度 | [2, 20]\n\
# ---- FEATURE: base ----\n\
base = Box(width, 24, height, align=(Align.CENTER, Align.CENTER, Align.MIN))\n\
# ---- FEATURE: slot ----\n\
slot = Pos(0, 0, height - 2) * Box(6, 30, 4, align=(Align.CENTER, Align.CENTER, Align.MIN))\n\
# ---- RESULT ----\n\
result = base - slot\n";

    fn fenced(code: &str) -> String {
        format!("```python\n{code}```")
    }

    fn metrics(size: [f64; 3]) -> CadMetrics {
        CadMetrics {
            bbox_min: [-size[0] / 2.0, -size[1] / 2.0, 0.0],
            bbox_max: [size[0] / 2.0, size[1] / 2.0, size[2]],
            size,
            volume_mm3: size[0] * size[1] * size[2],
            area_mm2: 1.0,
            solids: 1,
            faces: 10,
            edges: 24,
            is_valid: true,
        }
    }

    fn spec() -> DesignSpec {
        DesignSpec {
            name: "线缆夹".into(),
            summary: "桌面线缆夹".into(),
            suitable: true,
            unsuitable_reason: String::new(),
            overall_mm: [60.0, 24.0, 6.0],
            features: vec![
                DesignFeature {
                    name: "base".into(),
                    description: "底座".into(),
                    dimensions: [("length".to_string(), 60.0)].into(),
                },
                DesignFeature {
                    name: "slot".into(),
                    description: "线槽".into(),
                    dimensions: Default::default(),
                },
            ],
            assumptions: vec!["总长是估的".into()],
            print_notes: String::new(),
        }
    }

    fn script_error(message: &str, line: u32) -> CadError {
        CadError::Script(CadScriptError {
            stage: "exec".into(),
            error_type: "ValueError".into(),
            message: message.into(),
            line: Some(line),
            traceback: String::new(),
        })
    }

    /// 按脚本依次给出执行结果,并记下每次收到的代码。
    struct FakeExec {
        outcomes: Mutex<VecDeque<Result<CadMetrics, CadError>>>,
        seen: Mutex<Vec<String>>,
    }

    impl FakeExec {
        fn new(outcomes: impl IntoIterator<Item = Result<CadMetrics, CadError>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CadExecutor for FakeExec {
        async fn execute(&self, code: &str) -> Result<CadMetrics, CadError> {
            self.seen.lock().unwrap().push(code.to_string());
            self.outcomes.lock().unwrap().pop_front().expect("执行次数超出了测试脚本")
        }
    }

    fn quiet() -> impl Fn(CadProgress) + Send + Sync {
        |_| {}
    }

    // ---- 检查 ----

    #[test]
    fn a_clean_build_has_no_problems() {
        assert!(check_build(GOOD, &metrics([60.0, 24.0, 6.0]), Some(&spec()), [256.0; 3]).is_empty());
    }

    #[test]
    fn size_check_tolerates_5_percent_and_a_90_degree_turn_but_not_a_wrong_height() {
        let s = spec();
        assert!(check_build(GOOD, &metrics([61.5, 24.5, 6.4]), Some(&s), [256.0; 3]).is_empty(), "小尺寸按 1 mm、大尺寸按 5% 容差");
        assert!(check_build(GOOD, &metrics([24.0, 60.0, 6.0]), Some(&s), [256.0; 3]).is_empty(), "X / Y 互换不算错");
        let tall = check_build(GOOD, &metrics([60.0, 24.0, 12.0]), Some(&s), [256.0; 3]);
        assert!(matches!(tall.as_slice(), [CadProblem::Size { .. }]), "{tall:?}");
        let swapped_z = check_build(GOOD, &metrics([6.0, 24.0, 60.0]), Some(&s), [256.0; 3]);
        assert!(matches!(swapped_z.as_slice(), [CadProblem::Size { .. }]), "立起来打是另一回事:{swapped_z:?}");
        // 没有规格(指令修补)就不查尺寸
        assert!(check_build(GOOD, &metrics([60.0, 24.0, 12.0]), None, [256.0; 3]).is_empty());
    }

    #[test]
    fn geometry_problems_are_each_reported() {
        let mut m = metrics([300.0, 24.0, 6.0]);
        m.solids = 3;
        m.is_valid = false;
        m.bbox_min[2] = -3.0;
        let kinds: Vec<String> = check_build(GOOD, &m, None, [256.0; 3])
            .iter()
            .map(|p| serde_json::to_value(p).unwrap()["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["solids", "invalid", "off_plate", "too_big"]);
        // 长条斜着放不进去,但转 90° 能放进去的不算超限
        assert!(check_build(GOOD, &metrics([100.0, 250.0, 6.0]), None, [256.0, 120.0, 256.0]).is_empty());
    }

    #[test]
    fn contract_and_param_problems_are_reported_even_when_the_geometry_is_fine() {
        let no_sections = "from build123d import *\nresult = Box(60, 24, 6)\n";
        let problems = check_build(no_sections, &metrics([60.0, 24.0, 6.0]), None, [256.0; 3]);
        assert_eq!(problems.iter().filter(|p| matches!(p, CadProblem::Contract { .. })).count(), 2);
        assert!(problems.contains(&CadProblem::NoParams));

        let out_of_range = GOOD.replace("width = 60.0  # mm | 总宽 | [20, 200]", "width = 60.0  # mm | 总宽 | [80, 200]");
        let problems = check_build(&out_of_range, &metrics([60.0, 24.0, 6.0]), None, [256.0; 3]);
        assert!(matches!(problems.as_slice(), [CadProblem::ParamRange { name, .. }] if name == "width"), "{problems:?}");
    }

    // ---- 生成 ----

    #[tokio::test]
    async fn a_good_first_answer_is_delivered_without_repairs() {
        let llm = MockLlm::new([fenced(GOOD)]);
        let exec = FakeExec::new([Ok(metrics([60.0, 24.0, 6.0]))]);
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap();

        assert_eq!(built.code, GOOD);
        assert!(built.metrics.is_some());
        assert_eq!((built.report.llm_calls, built.report.runs), (1, 1));
        assert!(built.report.rounds.is_empty() && built.report.warnings.is_empty());
        assert!(built.report.cost_fen > 0.0);

        let req = &llm.requests()[0];
        assert_eq!(req.model, "deepseek-v4-pro", "写代码用最强的模型");
        assert!(req.thinking && !req.json);
        assert!(req.messages[0].content.contains("# ---- PARAMS ----"), "系统提示词里有代码契约");
        let user = &req.messages[1].content;
        assert!(user.contains("X 60 x Y 24 x Z 6") && user.contains("1. base — 底座") && user.contains("length = 60"));
        assert!(user.contains("总长是估的"), "规格里的假设也要交给写代码的模型");
    }

    #[tokio::test]
    async fn a_runtime_error_is_fed_back_with_its_line_and_the_fix_is_delivered() {
        let broken = GOOD.replace("result = base - slot", "result = fillet(base.edges(), radius=50)");
        let llm = MockLlm::new([fenced(&broken), fenced(GOOD)]);
        let exec = FakeExec::new([Err(script_error("Failed creating a fillet", 10)), Ok(metrics([60.0, 24.0, 6.0]))]);
        let seen_progress = Mutex::new(Vec::new());
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &|p| seen_progress.lock().unwrap().push(p))
            .await
            .unwrap();

        assert_eq!(built.code, GOOD);
        assert_eq!((built.report.llm_calls, built.report.runs), (2, 2));
        assert_eq!(built.report.rounds.len(), 1);
        assert!(built.report.warnings.is_empty());

        let second = &llm.requests()[1];
        assert_eq!(second.messages.len(), 4, "系统 + 规格 + 上一版回答 + 问题清单");
        let fix_request = &second.messages[3].content;
        assert!(fix_request.contains("[exec] ValueError: Failed creating a fillet (line 10)"), "{fix_request}");
        assert!(fix_request.contains("COMPLETE corrected script"));
        assert_eq!(
            *seen_progress.lock().unwrap(),
            [
                CadProgress::WritingCode { attempt: 1 },
                CadProgress::Running { attempt: 1 },
                CadProgress::Repairing { attempt: 1, problems: 1 },
                CadProgress::WritingCode { attempt: 2 },
                CadProgress::Running { attempt: 2 },
                CadProgress::Done,
            ]
        );
    }

    #[tokio::test]
    async fn loose_pieces_and_wrong_sizes_are_sent_back_even_though_the_script_ran() {
        let llm = MockLlm::new([fenced(GOOD), fenced(GOOD)]);
        let mut loose = metrics([60.0, 24.0, 12.0]);
        loose.solids = 2;
        let exec = FakeExec::new([Ok(loose), Ok(metrics([60.0, 24.0, 6.0]))]);
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap();

        assert!(built.report.warnings.is_empty());
        assert_eq!(built.report.rounds[0].len(), 2);
        let fix_request = &llm.requests()[1].messages[3].content;
        assert!(fix_request.contains("2 separate solids") && fix_request.contains("60.0 x 24.0 x 12.0"), "{fix_request}");
    }

    #[tokio::test]
    async fn when_repairs_run_out_the_least_bad_build_is_delivered_with_its_warnings() {
        let cfg = CadConfig {
            max_repairs: 2,
            ..Default::default()
        };
        let llm = MockLlm::new([fenced(GOOD), fenced(GOOD), fenced(GOOD)]);
        let mut two_problems = metrics([60.0, 24.0, 12.0]);
        two_problems.solids = 2;
        let exec = FakeExec::new([
            Ok(two_problems),
            Ok(metrics([60.0, 24.0, 9.0])), // 只剩尺寸不对
            Err(script_error("boom", 3)),   // 越修越坏
        ]);
        let built = generate_model(&llm, &exec, &spec(), &cfg, &quiet()).await.unwrap();

        assert_eq!(built.report.llm_calls, 3, "第一版 + 2 次修复,到此为止");
        assert_eq!(built.metrics.as_ref().unwrap().size[2], 9.0, "交付问题最少的那一版,不是最后一版");
        assert!(matches!(built.report.warnings.as_slice(), [CadProblem::Size { .. }]));
        assert_eq!(built.report.rounds.len(), 3);
    }

    #[tokio::test]
    async fn if_nothing_ever_runs_the_last_code_and_the_errors_come_back_instead_of_a_bare_failure() {
        let cfg = CadConfig {
            max_repairs: 1,
            ..Default::default()
        };
        let llm = MockLlm::new([fenced("result = Bx(1)\n"), fenced("result = Boxx(1)\n")]);
        let exec = FakeExec::new([Err(script_error("name 'Bx' is not defined", 1)), Err(script_error("name 'Boxx' is not defined", 1))]);
        let built = generate_model(&llm, &exec, &spec(), &cfg, &quiet()).await.unwrap();

        assert!(built.metrics.is_none());
        assert_eq!(built.code, "result = Boxx(1)\n", "留着最后一版,用户可以接着手改");
        assert_eq!(built.report.rounds.len(), 2);
        // 契约问题和运行错误在同一轮里一起报
        assert!(built.report.rounds[0].iter().any(|p| matches!(p, CadProblem::Contract { .. })));
        assert!(built.report.rounds[0].iter().any(|p| matches!(p, CadProblem::Script { .. })));
    }

    #[tokio::test]
    async fn a_part_floating_off_the_plate_is_snapped_down_without_another_model_call() {
        let llm = MockLlm::new([fenced(GOOD)]);
        let mut floating = metrics([60.0, 24.0, 6.0]);
        floating.bbox_min[2] = -3.0;
        let exec = FakeExec::new([Ok(floating), Ok(metrics([60.0, 24.0, 6.0]))]);
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap();

        assert_eq!((built.report.llm_calls, built.report.runs), (1, 2));
        assert!(built.report.plate_snapped && built.report.warnings.is_empty());
        assert!(built.code.trim_end().ends_with("# 贴到打印床 (z = 0)"));
        assert!(exec.seen()[1].contains("result.bounding_box().min.Z"));
        // 补的那一行不能破坏契约:参数照样解析得出来
        assert_eq!(parse_params(&built.code).len(), 2);
    }

    #[tokio::test]
    async fn a_timeout_is_something_the_model_can_fix_but_a_missing_engine_is_not() {
        let llm = MockLlm::new([fenced(GOOD), fenced(GOOD)]);
        let exec = FakeExec::new([Err(CadError::Timeout(Duration::from_secs(90))), Ok(metrics([60.0, 24.0, 6.0]))]);
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap();
        assert!(matches!(built.report.rounds[0].as_slice(), [CadProblem::Timeout { secs: 90 }]));
        assert!(llm.requests()[1].messages[3].content.contains("longer than 90 s"));

        let llm = MockLlm::new([fenced(GOOD)]);
        let exec = FakeExec::new([Err(CadError::EngineMissing)]);
        let err = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap_err();
        assert_eq!(err, AgentError::Engine(CadError::EngineMissing));
        assert_eq!(err.code(), "cad_engine_missing");
    }

    #[tokio::test]
    async fn a_provider_failure_mid_repair_does_not_throw_away_a_build_that_already_works() {
        let llm = MockLlm::new([fenced(GOOD)]).then_fail(ProviderError::RateLimited);
        let exec = FakeExec::new([Ok(metrics([60.0, 24.0, 9.0]))]);
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap();
        assert!(built.metrics.is_some());
        assert!(matches!(built.report.warnings.as_slice(), [CadProblem::Size { .. }]));

        // 但第一版就拿不到回答,那就是失败
        let llm = MockLlm::new(Vec::<String>::new()).then_fail(ProviderError::Auth);
        let exec = FakeExec::new([]);
        let err = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap_err();
        assert_eq!(err, AgentError::Provider(ProviderError::Auth));
    }

    #[tokio::test]
    async fn an_empty_answer_is_asked_again_once() {
        let llm = MockLlm::new(Vec::<String>::new())
            .then_fail(ProviderError::BadResponse("empty content".into()))
            .then_answer(fenced(GOOD));
        let exec = FakeExec::new([Ok(metrics([60.0, 24.0, 6.0]))]);
        let built = generate_model(&llm, &exec, &spec(), &CadConfig::default(), &quiet()).await.unwrap();
        assert_eq!(built.report.llm_calls, 2);
        assert!(built.report.rounds.is_empty());
    }

    // ---- 指令修补 ----

    #[tokio::test]
    async fn an_edit_reports_exactly_which_sections_changed() {
        let edited = GOOD.replace("Box(6, 30, 4,", "Box(8, 30, 4,");
        let llm = MockLlm::new([fenced(&edited)]);
        let exec = FakeExec::new([Ok(metrics([60.0, 24.0, 6.0]))]);
        let pick = CadPick {
            point: [0.0, 0.0, 6.0],
            normal: Some([0.0, 0.0, 1.0]),
        };
        let built = edit_model(&llm, &exec, GOOD, "线槽加宽到 8 mm", Some(pick), &CadConfig::default(), &quiet())
            .await
            .unwrap();

        assert_eq!(built.code, edited);
        assert_eq!(built.report.changed_sections, ["slot"]);

        let req = &llm.requests()[0];
        let system = &req.messages[0].content;
        assert!(system.contains("LOCAL modification") && system.contains("cheat sheet"), "修补提示词 + 完整的建模提示词");
        let user = &req.messages[1].content;
        assert!(user.contains("线槽加宽到 8 mm") && user.contains("PARAMS, base, slot, RESULT"));
        assert!(user.contains("point = (0.00, 0.00, 6.00) mm, face normal = (0.00, 0.00, 1.00)"));
        assert!(user.contains("result = base - slot"), "要把现有脚本完整地交给模型");
    }

    #[tokio::test]
    async fn an_unchanged_script_is_sent_back_without_wasting_an_engine_run() {
        let edited = GOOD.replace("height = 6.0", "height = 8.0");
        // 第一次原样交回(只是行尾多了空格),第二次才真的改
        let llm = MockLlm::new([fenced(&GOOD.replace("result = base - slot", "result = base - slot  ")), fenced(&edited)]);
        let exec = FakeExec::new([Ok(metrics([60.0, 24.0, 8.0]))]);
        let built = edit_model(&llm, &exec, GOOD, "厚度改成 8", None, &CadConfig::default(), &quiet()).await.unwrap();

        assert_eq!(built.report.runs, 1, "没改的那一版不用跑");
        assert_eq!(built.report.rounds[0], [CadProblem::Unchanged]);
        assert_eq!(built.report.changed_sections, ["PARAMS"]);
        assert!(llm.requests()[1].messages[3].content.contains("returned the script unchanged"));
    }

    #[tokio::test]
    async fn an_edit_does_not_hold_the_part_to_the_old_spec_size() {
        let edited = GOOD.replace("height = 6.0", "height = 12.0");
        let llm = MockLlm::new([fenced(&edited)]);
        let exec = FakeExec::new([Ok(metrics([60.0, 24.0, 12.0]))]);
        let built = edit_model(&llm, &exec, GOOD, "加厚一倍", None, &CadConfig::default(), &quiet()).await.unwrap();
        assert!(built.report.warnings.is_empty() && built.report.rounds.is_empty());
    }

    #[tokio::test]
    async fn blank_instructions_are_refused_before_spending_anything() {
        let llm = MockLlm::new(Vec::<String>::new());
        let exec = FakeExec::new([]);
        let err = edit_model(&llm, &exec, GOOD, "  ", None, &CadConfig::default(), &quiet()).await.unwrap_err();
        assert_eq!(err, AgentError::EmptyBrief);
        assert_eq!(llm.calls(), 0);
    }

    // ---- 看图 ----

    const SPEC_JSON: &str = r#"{"name":"线缆夹","summary":"桌面线缆夹","suitable":true,"overall_mm":[60,24,18],
"features":[{"name":"base","description":"底座","dimensions":{"length":60,"width":24,"height":6}}],
"assumptions":["尺寸是估的"],"print_notes":"无需支撑"}"#;

    #[tokio::test]
    async fn the_spec_comes_from_the_vision_model_with_the_images_attached() {
        let llm = MockLlm::new([SPEC_JSON]);
        let images = vec!["data:image/jpeg;base64,AAAA".to_string()];
        let (spec, report) = design_spec(&llm, "三道线槽", &images, &CadConfig::default(), &quiet()).await.unwrap();

        assert_eq!(spec.overall_mm, [60.0, 24.0, 18.0]);
        assert_eq!(spec.features[0].dimensions["height"], 6.0);
        assert_eq!(report.llm_calls, 1);
        assert_eq!(report.runs, 0);

        let req = &llm.requests()[0];
        assert_eq!(req.model, "deepseek-flash", "只有 flash 看得见图");
        assert!(req.json && req.thinking);
        assert!(req.messages[0].content.contains("256 × 256 × 256"), "成型空间要写进提示词");
        assert!(req.messages[0].content.contains("json"), "DeepSeek 的 JSON 模式要求提示词里出现 json 这个词");
        assert_eq!(req.messages[1].images, images);
        assert!(req.messages[1].content.contains("三道线槽"));
    }

    #[tokio::test]
    async fn a_spec_with_nonsense_dimensions_is_sent_back_once() {
        let bad = SPEC_JSON.replace("[60,24,18]", "[60,0,18]");
        let llm = MockLlm::new([bad.as_str(), SPEC_JSON]);
        let (spec, report) = design_spec(&llm, "线缆夹", &[], &CadConfig::default(), &quiet()).await.unwrap();
        assert_eq!(spec.name, "线缆夹");
        assert_eq!(report.llm_calls, 2);
        assert!(llm.requests()[1].messages[3].content.contains("overall_mm"));
    }

    #[tokio::test]
    async fn an_organic_shape_is_reported_as_unsuitable_instead_of_being_forced() {
        let llm = MockLlm::new([r#"{"name":"小猫摆件","suitable":false,"unsuitable_reason":"有机造型,适合走 AI 网格生成","overall_mm":[50,40,60],"features":[]}"#]);
        let (spec, _) = design_spec(&llm, "", &["data:image/png;base64,AA".to_string()], &CadConfig::default(), &quiet())
            .await
            .unwrap();
        assert!(!spec.suitable);
        assert!(spec.unsuitable_reason.contains("有机"));

        // 没有特征的规格不能拿去生成
        let exec = FakeExec::new([]);
        let err = generate_model(&llm, &exec, &spec, &CadConfig::default(), &quiet()).await.unwrap_err();
        assert!(matches!(err, AgentError::InvalidOutput(_)));
    }

    #[tokio::test]
    async fn nothing_to_look_at_and_nothing_to_read_is_refused() {
        let llm = MockLlm::new(Vec::<String>::new());
        let err = design_spec(&llm, " ", &[], &CadConfig::default(), &quiet()).await.unwrap_err();
        assert_eq!(err, AgentError::EmptyBrief);
    }

    #[tokio::test]
    async fn the_review_sees_reference_images_first_then_the_renders() {
        let llm = MockLlm::new([r#"{"matches":false,"differences":["顶面少了第三道线槽,应为等距的 3 道",""," "]}"#]);
        let refs = vec!["data:ref".to_string()];
        let renders = vec!["data:iso".to_string(), "data:top".to_string()];
        let (review, report) = review_model(&llm, &refs, &renders, Some(&spec()), &CadConfig::default(), &quiet())
            .await
            .unwrap();

        assert!(!review.matches);
        assert_eq!(review.differences, ["顶面少了第三道线槽,应为等距的 3 道"], "空行要清掉");
        assert_eq!(report.llm_calls, 1);
        let msg = &llm.requests()[0].messages[1];
        assert_eq!(msg.images, ["data:ref", "data:iso", "data:top"]);
        assert!(msg.content.contains("1 张参考图") && msg.content.contains("2 张模型渲染图"));
    }

    #[tokio::test]
    async fn a_review_that_says_no_without_saying_why_is_sent_back() {
        let llm = MockLlm::new([r#"{"matches":false,"differences":[]}"#, r#"{"matches":true,"differences":[]}"#]);
        let (review, report) = review_model(&llm, &["data:a".to_string()], &["data:b".to_string()], None, &CadConfig::default(), &quiet())
            .await
            .unwrap();
        assert!(review.matches);
        assert_eq!(report.llm_calls, 2);
    }
}
