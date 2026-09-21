//! 对话式建模:用户对着当前模型说一句话,AI 自己判断是「改模型」「回答问题」还是「反问澄清」。
//!
//! 一次模型调用同时完成「判断意图」和「干活」:回答里有 ```python 围栏 = 改了模型(整份脚本),
//! 没有 = 纯文字(回答 / 反问)。改模型的那一支接着走 `cad::build_loop`:真引擎执行 → 检查 → 不合格带着报错修。
//!
//! 上下文怎么给:
//! - **当前脚本永远随消息一起给**——用户可能刚手动改过参数,脚本才是事实;
//! - 之前的对话只给文字(不给历次脚本),并且只留最近的若干句:省 token,也避免模型照着旧脚本改;
//! - 点选的位置、对后来贴的图的文字说明(写代码的模型看不见图,由视觉模型先描述)附在这一句后面。

use std::time::Instant;

use pp_cad::contract::sections;
use pp_common::cad::{CadBuildReport, CadPick, DesignSpec};
use pp_providers::llm::{ChatMessage, ChatRequest, LlmProvider, Usage};

use crate::cad::{build_loop, chat_once, CadBuild, CadConfig, CadExecutor, CadProgress, FirstAnswer, CODE_PROMPT};
use crate::AgentError;

const CHAT_PROMPT: &str = include_str!("../prompts/cad_chat.md");

/// 带进上下文的历史:最多这么多句、每句最多这么多字、总共最多这么多字。
const HISTORY_LINES: usize = 12;
const HISTORY_LINE_CHARS: usize = 400;
const HISTORY_TOTAL_CHARS: usize = 4000;

/// 之前对话里的一句(只有文字)。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatLine {
    pub from_user: bool,
    pub text: String,
}

pub struct ChatTurn<'a> {
    pub history: &'a [ChatLine],
    /// 当前选中版本的脚本
    pub code: &'a str,
    pub spec: Option<&'a DesignSpec>,
    pub message: &'a str,
    pub pick: Option<CadPick>,
    /// 视觉模型对「这一句里贴的图」的描述
    pub image_notes: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatOutcome {
    /// 没动模型:回答了问题,或者反问了一句
    Reply { text: String, report: CadBuildReport },
    /// 动了模型。`text` 是 AI 对改动的一句话说明;`build.metrics` 为空表示一版能跑的都没有
    Build { text: String, build: CadBuild },
}

/// 把回答拆成「围栏外的话」和「围栏里的脚本」。
pub fn split_answer(answer: &str) -> (String, Option<String>) {
    let Some(start) = answer.find("```") else {
        return (answer.trim().to_string(), None);
    };
    let after = &answer[start + 3..];
    let end = after.find("```");
    let code = pp_cad::contract::extract_code(answer);
    let mut text = answer[..start].trim().to_string();
    if let Some(end) = end {
        let tail = after[end + 3..].trim();
        if !tail.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(tail);
        }
    }
    // 围栏里要像一份脚本才算「改了模型」;回答里顺手贴的一两行示例不算
    let looks_like_script = code.contains("result") || code.contains("build123d") || code.lines().count() >= 6;
    if looks_like_script {
        (text, Some(code))
    } else {
        (answer.trim().to_string(), None)
    }
}

fn clip(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 最近的若干句 → 交替的 user / assistant 消息(相邻同角色的合并,有些接口不接受连续两条同角色)。
fn history_messages(history: &[ChatLine]) -> Vec<ChatMessage> {
    let mut picked: Vec<&ChatLine> = Vec::new();
    let mut budget = HISTORY_TOTAL_CHARS;
    for line in history.iter().rev().take(HISTORY_LINES) {
        let cost = line.text.chars().count().min(HISTORY_LINE_CHARS);
        if cost > budget {
            break;
        }
        budget -= cost;
        picked.push(line);
    }
    picked.reverse();
    // 历史要从用户的话开始
    while picked.first().is_some_and(|l| !l.from_user) {
        picked.remove(0);
    }

    let mut out: Vec<ChatMessage> = Vec::new();
    for line in picked {
        let text = clip(line.text.trim(), HISTORY_LINE_CHARS);
        if text.is_empty() {
            continue;
        }
        let same_role = out.last().is_some_and(|m| (m.role == pp_providers::llm::Role::User) == line.from_user);
        match out.last_mut() {
            Some(last) if same_role => {
                last.content.push('\n');
                last.content.push_str(&text);
            }
            _ => out.push(if line.from_user { ChatMessage::user(text) } else { ChatMessage::assistant(text) }),
        }
    }
    out
}

fn turn_message(turn: &ChatTurn<'_>) -> String {
    let names: Vec<String> = sections(turn.code).into_iter().map(|s| s.name).collect();
    let mut out = format!(
        "## Current script\n```python\n{}\n```\n\n## Sections\n{}\n",
        turn.code.trim_end(),
        names.join(", ")
    );
    if let Some(spec) = turn.spec {
        out.push_str(&format!(
            "\n## What this part is\n{} — {} (spec size {} x {} x {} mm)\n",
            spec.name.trim(),
            spec.summary.trim(),
            spec.overall_mm[0],
            spec.overall_mm[1],
            spec.overall_mm[2]
        ));
    }
    if let Some(p) = turn.pick {
        out.push_str(&format!(
            "\n## Picked location\npoint = ({:.2}, {:.2}, {:.2}) mm",
            p.point[0], p.point[1], p.point[2]
        ));
        if let Some(n) = p.normal {
            out.push_str(&format!(", face normal = ({:.2}, {:.2}, {:.2})", n[0], n[1], n[2]));
        }
        out.push('\n');
    }
    if let Some(notes) = turn.image_notes.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(&format!("\n## Notes about the attached images\n{notes}\n"));
    }
    out.push_str(&format!("\n## Message\n{}\n", turn.message.trim()));
    out
}

/// 一轮对话。
pub async fn chat_turn(
    llm: &dyn LlmProvider,
    exec: &dyn CadExecutor,
    turn: ChatTurn<'_>,
    cfg: &CadConfig,
    on_progress: &(dyn Fn(CadProgress) + Send + Sync),
) -> Result<ChatOutcome, AgentError> {
    if turn.message.trim().is_empty() || turn.code.trim().is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    let mut messages = vec![ChatMessage::system(format!("{CHAT_PROMPT}\n---\n\n{CODE_PROMPT}"))];
    messages.extend(history_messages(turn.history));
    messages.push(ChatMessage::user(turn_message(&turn)));

    on_progress(CadProgress::WritingCode { attempt: 1 });
    let req = ChatRequest::new(&cfg.code_model, messages.clone())
        .thinking(cfg.code_thinking)
        .max_tokens(cfg.code_max_tokens);
    let mut calls = 0;
    let answer = chat_once(llm, &req, &mut calls).await?;
    let (text, code) = split_answer(&answer.content);

    if code.is_none() {
        on_progress(CadProgress::Done);
        let usage: Usage = answer.usage;
        return Ok(ChatOutcome::Reply {
            text,
            report: CadBuildReport {
                llm_calls: calls,
                tokens_in: usage.prompt_tokens,
                tokens_out: usage.completion_tokens,
                cost_fen: cfg.code_pricing.cost_fen(usage),
                elapsed_ms: started.elapsed().as_millis() as u64,
                ..Default::default()
            },
        });
    }

    // 改了模型:接着走「执行 → 检查 → 修」。第一版回答已经在手里了,不再重问
    let first = FirstAnswer { response: answer, calls };
    let mut build = build_loop(llm, exec, messages, None, Some(turn.code), Some(first), cfg, on_progress).await?;
    build.report.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(ChatOutcome::Build { text, build })
}

/// 写代码的模型看不见图。用户在对话中途贴的图,先让视觉模型结合这句话描述一下「图里与这次修改有关的东西」。
pub async fn describe_images(
    llm: &dyn LlmProvider,
    images: &[String],
    message: &str,
    cfg: &CadConfig,
) -> Result<(String, CadBuildReport), AgentError> {
    if images.is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    let system = "你在帮一位 CAD 建模师看图。用户正在和建模助手对话修改一个 3D 打印零件,并随这句话贴了图。\
请用不超过 150 字描述图里与这句话有关的形体信息:形状、相对位置、数量、比例、能估出的尺寸(毫米)。\
只描述看得见的形体,不要评价,不要给建模步骤,不要输出代码。";
    let req = ChatRequest::new(
        &cfg.vision_model,
        vec![
            ChatMessage::system(system),
            ChatMessage::user_with_images(format!("用户这句话:{}", message.trim()), images.to_vec()),
        ],
    )
    .thinking(false)
    .max_tokens(600);
    let mut calls = 0;
    let answer = chat_once(llm, &req, &mut calls).await?;
    let report = CadBuildReport {
        llm_calls: calls,
        tokens_in: answer.usage.prompt_tokens,
        tokens_out: answer.usage.completion_tokens,
        cost_fen: cfg.vision_pricing.cost_fen(answer.usage),
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..Default::default()
    };
    Ok((answer.content.trim().to_string(), report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use pp_cad::CadError;
    use pp_common::cad::{CadMetrics, CadProblem, CadScriptError};
    use pp_providers::llm::MockLlm;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const CODE: &str = "from build123d import *\n\
# ---- PARAMS ----\n\
slot_width = 6.0  # mm | 线槽宽度 | [2, 15]\n\
# ---- FEATURE: body ----\n\
body = Box(60, 24, 16, align=(Align.CENTER, Align.CENTER, Align.MIN))\n\
# ---- FEATURE: slot ----\n\
body = body - Pos(0, 0, 8) * Box(slot_width, 30, 10, align=(Align.CENTER, Align.CENTER, Align.MIN))\n\
# ---- RESULT ----\n\
result = body\n";

    struct FakeExec(Mutex<VecDeque<Result<CadMetrics, CadError>>>, Mutex<u32>);

    impl FakeExec {
        fn new(outcomes: impl IntoIterator<Item = Result<CadMetrics, CadError>>) -> Self {
            Self(Mutex::new(outcomes.into_iter().collect()), Mutex::new(0))
        }
        fn runs(&self) -> u32 {
            *self.1.lock().unwrap()
        }
    }

    #[async_trait]
    impl CadExecutor for FakeExec {
        async fn execute(&self, _code: &str) -> Result<CadMetrics, CadError> {
            *self.1.lock().unwrap() += 1;
            self.0.lock().unwrap().pop_front().expect("执行次数超出了测试脚本")
        }
    }

    fn ok_metrics() -> CadMetrics {
        CadMetrics {
            bbox_min: [-30.0, -12.0, 0.0],
            bbox_max: [30.0, 12.0, 16.0],
            size: [60.0, 24.0, 16.0],
            volume_mm3: 1.0,
            area_mm2: 1.0,
            solids: 1,
            faces: 10,
            edges: 24,
            is_valid: true,
        }
    }

    fn turn<'a>(history: &'a [ChatLine], message: &'a str) -> ChatTurn<'a> {
        ChatTurn {
            history,
            code: CODE,
            spec: None,
            message,
            pick: None,
            image_notes: None,
        }
    }

    fn quiet() -> impl Fn(CadProgress) + Send + Sync {
        |_| {}
    }

    #[test]
    fn answers_are_split_into_prose_and_script() {
        let (text, code) = split_answer("线槽加宽到 8 mm。\n```python\nfrom build123d import *\nresult = Box(1, 1, 1)\n```\n其余不变。");
        assert_eq!(text, "线槽加宽到 8 mm。\n其余不变。");
        assert_eq!(code.as_deref(), Some("from build123d import *\nresult = Box(1, 1, 1)\n"));

        let (text, code) = split_answer("不需要支撑:线槽开口朝上,没有悬垂。");
        assert_eq!((text.as_str(), code), ("不需要支撑:线槽开口朝上,没有悬垂。", None));

        // 回答问题时顺手贴的一行示例,不算改模型
        let (text, code) = split_answer("壁厚由这一行决定:\n```python\nwall = 2.0\n```");
        assert!(code.is_none());
        assert!(text.contains("wall = 2.0"), "整段话原样留着:{text}");
    }

    #[tokio::test]
    async fn a_question_gets_an_answer_without_touching_the_engine() {
        let llm = MockLlm::new(["不需要支撑:线槽开口朝上,没有超过 45° 的悬垂。"]);
        let exec = FakeExec::new([]);
        let out = chat_turn(&llm, &exec, turn(&[], "这个打印要加支撑吗?"), &CadConfig::default(), &quiet())
            .await
            .unwrap();
        let ChatOutcome::Reply { text, report } = out else { panic!("应当是纯文字回答") };
        assert!(text.contains("不需要支撑"));
        assert_eq!((report.llm_calls, report.runs, exec.runs()), (1, 0, 0));
        assert!(report.cost_fen > 0.0);
    }

    #[tokio::test]
    async fn a_change_request_runs_the_new_script_and_reports_what_changed() {
        let edited = CODE.replace("slot_width = 6.0", "slot_width = 8.0");
        let llm = MockLlm::new([format!("线槽从 6 mm 加宽到 8 mm,其余不变。\n```python\n{edited}```")]);
        let exec = FakeExec::new([Ok(ok_metrics())]);
        let history = [
            ChatLine { from_user: true, text: "做一个线缆夹".into() },
            ChatLine { from_user: false, text: "已生成:60 × 24 × 16 mm,一道线槽。".into() },
        ];
        let mut t = turn(&history, "线槽再宽一点");
        t.pick = Some(CadPick {
            point: [0.0, 0.0, 16.0],
            normal: Some([0.0, 0.0, 1.0]),
        });
        t.image_notes = Some("图里的线槽明显比现在宽,大约是总长的 1/7。");
        let out = chat_turn(&llm, &exec, t, &CadConfig::default(), &quiet()).await.unwrap();

        let ChatOutcome::Build { text, build } = out else { panic!("应当改了模型") };
        assert_eq!(text, "线槽从 6 mm 加宽到 8 mm,其余不变。");
        assert_eq!(build.code, edited);
        assert!(build.metrics.is_some());
        assert_eq!(build.report.changed_sections, ["PARAMS"]);
        assert_eq!((build.report.llm_calls, build.report.runs), (1, 1), "第一版回答不重复调用模型");

        let req = &llm.requests()[0];
        assert!(req.messages[0].content.contains("exactly one of three forms") && req.messages[0].content.contains("cheat sheet"));
        assert_eq!(req.messages.len(), 4, "系统 + 两句历史 + 这一句");
        assert_eq!(req.messages[1].content, "做一个线缆夹");
        let last = &req.messages[3].content;
        assert!(last.contains("slot_width = 6.0") && last.contains("PARAMS, body, slot, RESULT"));
        assert!(last.contains("point = (0.00, 0.00, 16.00)") && last.contains("大约是总长的 1/7") && last.contains("线槽再宽一点"));
    }

    #[tokio::test]
    async fn a_broken_edit_is_repaired_inside_the_same_turn() {
        let broken = CODE.replace("result = body", "result = fillet(body.edges(), radius=50)");
        let fixed = CODE.replace("result = body", "result = fillet(body.edges().filter_by(Axis.Z), radius=2)");
        let llm = MockLlm::new([format!("四周加了圆角。\n```python\n{broken}```"), format!("```python\n{fixed}```")]);
        let err = CadError::Script(CadScriptError {
            stage: "exec".into(),
            error_type: "ValueError".into(),
            message: "Failed creating a fillet".into(),
            line: Some(9),
            traceback: String::new(),
        });
        let exec = FakeExec::new([Err(err), Ok(ok_metrics())]);
        let out = chat_turn(&llm, &exec, turn(&[], "四周加圆角"), &CadConfig::default(), &quiet()).await.unwrap();
        let ChatOutcome::Build { text, build } = out else { panic!() };
        assert_eq!(text, "四周加了圆角。", "说明取第一版回答里的那句话");
        assert_eq!(build.code, fixed);
        assert_eq!((build.report.llm_calls, build.report.runs, build.report.rounds.len()), (2, 2, 1));
        assert!(matches!(build.report.rounds[0][0], CadProblem::Script { .. }));
        assert!(llm.requests()[1].messages.last().unwrap().content.contains("Failed creating a fillet"));
    }

    #[test]
    fn only_recent_history_is_sent_and_it_starts_with_the_user() {
        let mut history = Vec::new();
        for i in 0..20 {
            history.push(ChatLine { from_user: true, text: format!("第 {i} 句") });
            history.push(ChatLine { from_user: false, text: format!("回答 {i}") });
        }
        let msgs = history_messages(&history);
        assert_eq!(msgs.len(), HISTORY_LINES);
        assert_eq!(msgs[0].role, pp_providers::llm::Role::User);
        assert_eq!(msgs.last().unwrap().content, "回答 19");

        // 连续两句用户的话合并成一条;超长的一句会被截断
        let merged = history_messages(&[
            ChatLine { from_user: true, text: "加个孔".into() },
            ChatLine { from_user: true, text: "x".repeat(1000) },
        ]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].content.starts_with("加个孔\n") && merged[0].content.chars().count() < 500);
    }

    #[tokio::test]
    async fn pasted_images_are_described_by_the_vision_model() {
        let llm = MockLlm::new(["图里是一个 U 形槽,槽底是半圆,半径约为槽宽的一半。"]);
        let (notes, report) = describe_images(&llm, &["data:image/jpeg;base64,AA".to_string()], "槽底改成这样", &CadConfig::default())
            .await
            .unwrap();
        assert!(notes.contains("U 形槽"));
        assert_eq!(report.llm_calls, 1);
        let req = &llm.requests()[0];
        assert_eq!(req.model, "deepseek-flash", "只有视觉模型看得见图");
        assert_eq!(req.messages[1].images.len(), 1);
        assert!(req.messages[1].content.contains("槽底改成这样"));
    }

    #[tokio::test]
    async fn nothing_to_say_or_nothing_to_edit_is_refused() {
        let llm = MockLlm::new(Vec::<String>::new());
        let exec = FakeExec::new([]);
        let err = chat_turn(&llm, &exec, turn(&[], "  "), &CadConfig::default(), &quiet()).await.unwrap_err();
        assert_eq!(err, AgentError::EmptyBrief);
        assert_eq!(llm.calls(), 0);
    }
}
