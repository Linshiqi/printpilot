//! 上架文案的起草(M5):按角度写笔记候选、写商品标题 / 卖点 / 详情。
//!
//! 和别的流水线一样是「有界」的:一次调用出全部候选(JSON 模式,不流式——半截 JSON 没法给人看);
//! 输出违反渠道的硬规格(标题超 20 字…)或命中违禁词时,带着原因**重问一次**;第二次仍有问题就照样交回去——
//! 界面上的合规检查会把问题标出来,人来改比再花一次调用划算。结构不对(篇数不够、空标题)才算失败。

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use pp_common::publish::{char_len, lint_listing, lint_note, LintField, ListingDraft, ModelLicense, NoteAngle, NoteDraft, Severity};
use pp_providers::llm::{ChatMessage, ChatRequest, LlmPricing, LlmProvider};
use serde::Deserialize;

use crate::json::ask_json;
use crate::AgentError;

const NOTE_PROMPT: &str = include_str!("../prompts/note_draft.md");
const LISTING_PROMPT: &str = include_str!("../prompts/listing_draft.md");

/// 写文案要知道的事。都是事实:调研、规格、成本模型里已经有的东西;没有的就留空,提示词要求「没给的不要编」。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublishBrief {
    pub product: String,
    pub category: String,
    /// 谁会买、为什么买(项目的假设)
    pub hypothesis: String,
    /// 一条一个事实:尺寸、材质、特征、价格、能不能定制…
    pub facts: Vec<String>,
    pub brand_voice: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublishConfig {
    pub model: String,
    pub pricing: LlmPricing,
}

impl Default for PublishConfig {
    fn default() -> Self {
        // 文案要的是语感,用强的那个;一次调用两三千 token,差价可以忽略
        Self {
            model: "deepseek-v4-pro".into(),
            pricing: LlmPricing::deepseek_pro(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteCandidate {
    pub angle: NoteAngle,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingCandidate {
    pub title: String,
    pub selling_points: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DraftReport {
    pub llm_calls: u32,
    pub cost_fen: f64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct RawNotes {
    notes: Vec<RawNote>,
}

#[derive(Debug, Deserialize)]
struct RawNote {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawListing {
    #[serde(default)]
    title: String,
    #[serde(default)]
    selling_points: Vec<String>,
    #[serde(default)]
    body: String,
}

fn brief_block(brief: &PublishBrief) -> String {
    let mut out = format!("## 产品信息\n- 名称:{}\n", brief.product.trim());
    if !brief.category.trim().is_empty() {
        out.push_str(&format!("- 品类:{}\n", brief.category.trim()));
    }
    if !brief.hypothesis.trim().is_empty() {
        out.push_str(&format!("- 谁会买、为什么买:{}\n", brief.hypothesis.trim()));
    }
    for fact in brief.facts.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
        out.push_str(&format!("- {fact}\n"));
    }
    let voice = brief.brand_voice.trim();
    out.push_str(&format!("\n## 品牌语气\n{}\n", if voice.is_empty() { "(没有给出)" } else { voice }));
    out
}

fn field_name(f: LintField) -> &'static str {
    match f {
        LintField::Title => "标题",
        LintField::Body => "正文",
        LintField::Tags => "话题",
        LintField::Images => "图片",
        LintField::Price => "价格",
        LintField::License => "授权",
    }
}

/// 文字上的硬伤(和图片、价格、授权无关的那些),写成给模型看的一句话。
fn text_problems(issues: &[pp_common::publish::LintIssue]) -> Vec<String> {
    issues
        .iter()
        .filter(|i| i.severity == Severity::Block && matches!(i.field, LintField::Title | LintField::Body | LintField::Tags))
        .map(|i| format!("{}「{}」({:?})", field_name(i.field), i.hit, i.code))
        .collect()
}

fn note_problems(n: &RawNote) -> Vec<String> {
    let draft = NoteDraft {
        title: n.title.clone(),
        body: n.body.clone(),
        tags: n.tags.clone(),
        ..Default::default()
    };
    text_problems(&lint_note(&draft, &[]))
}

/// 按角度各写一篇。`angles` 为空 → 默认写痛点和场景两篇(PRD:至少两个候选)。
pub async fn draft_notes(llm: &dyn LlmProvider, brief: &PublishBrief, angles: &[NoteAngle], cfg: &PublishConfig) -> Result<(Vec<NoteCandidate>, DraftReport), AgentError> {
    if brief.product.trim().is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    let angles: Vec<NoteAngle> = if angles.is_empty() { vec![NoteAngle::Pain, NoteAngle::Scene] } else { angles.to_vec() };
    let wanted = angles.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(", ");
    let user = format!("{}\n## 这次要写的角度(每个恰好一篇,按这个顺序)\n{wanted}\n", brief_block(brief));
    let req = ChatRequest::new(&cfg.model, vec![ChatMessage::system(NOTE_PROMPT), ChatMessage::user(user)]).thinking(false).max_tokens(4000);

    let attempt = AtomicU32::new(0);
    let expected = angles.len();
    let answer = ask_json::<RawNotes, _>(llm, req, |raw| {
        let first = attempt.fetch_add(1, Ordering::Relaxed) == 0;
        if raw.notes.len() < expected {
            return Err(format!("要 {expected} 篇,只给了 {} 篇", raw.notes.len()));
        }
        if let Some(i) = raw.notes.iter().position(|n| n.title.trim().is_empty() || n.body.trim().is_empty()) {
            return Err(format!("第 {} 篇的标题或正文是空的", i + 1));
        }
        // 规格和违禁词只在第一次卡:第二次还有问题就交给人改
        let problems: Vec<String> = raw.notes.iter().enumerate().flat_map(|(i, n)| note_problems(n).into_iter().map(move |p| format!("第 {} 篇 {p}", i + 1))).collect();
        if first && !problems.is_empty() {
            return Err(format!("{};请改掉这些地方(标题不超过 20 字、正文不超过 1000 字、话题不超过 5 个,不要出现违禁词)", problems.join(";")));
        }
        Ok(())
    })
    .await?;

    let notes = answer
        .value
        .notes
        .into_iter()
        .take(expected)
        .enumerate()
        .map(|(i, n)| NoteCandidate {
            // 顺序是我们定的:模型把角度名写错了也不要紧
            angle: angles[i],
            title: n.title.trim().to_string(),
            body: n.body.trim().to_string(),
            tags: n.tags.iter().map(|t| t.trim().trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect(),
        })
        .collect();
    let report = DraftReport {
        llm_calls: answer.calls,
        cost_fen: cfg.pricing.cost_fen(answer.usage),
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    Ok((notes, report))
}

/// 写商品标题、卖点、详情。
pub async fn draft_listing(llm: &dyn LlmProvider, brief: &PublishBrief, cfg: &PublishConfig) -> Result<(ListingCandidate, DraftReport), AgentError> {
    if brief.product.trim().is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    let req = ChatRequest::new(&cfg.model, vec![ChatMessage::system(LISTING_PROMPT), ChatMessage::user(brief_block(brief))]).thinking(false).max_tokens(3000);
    let attempt = AtomicU32::new(0);
    let answer = ask_json::<RawListing, _>(llm, req, |raw| {
        let first = attempt.fetch_add(1, Ordering::Relaxed) == 0;
        if raw.title.trim().is_empty() || (raw.body.trim().is_empty() && raw.selling_points.is_empty()) {
            return Err("标题和详情不能是空的".into());
        }
        let draft = ListingDraft {
            title: raw.title.clone(),
            selling_points: raw.selling_points.clone(),
            body: raw.body.clone(),
            // 只查文字:价格、授权、图片由人在界面上填
            price_yuan: Some(1.0),
            model_license: Some(ModelLicense::Original),
            ..Default::default()
        };
        let mut problems = text_problems(&lint_listing(&draft, &[], None));
        if let Some(long) = raw.selling_points.iter().find(|p| char_len(p) > 30) {
            problems.push(format!("卖点太长:「{long}」"));
        }
        if first && !problems.is_empty() {
            return Err(format!("{};请改掉这些地方", problems.join(";")));
        }
        Ok(())
    })
    .await?;
    let raw = answer.value;
    let candidate = ListingCandidate {
        title: raw.title.trim().to_string(),
        selling_points: raw.selling_points.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).take(6).collect(),
        body: raw.body.trim().to_string(),
    };
    let report = DraftReport {
        llm_calls: answer.calls,
        cost_fen: cfg.pricing.cost_fen(answer.usage),
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    Ok((candidate, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_providers::llm::MockLlm;

    fn brief() -> PublishBrief {
        PublishBrief {
            product: "磁吸线缆夹".into(),
            category: "桌面收纳".into(),
            hypothesis: "桌面线多的上班族,愿意花 30 元让桌面清爽".into(),
            facts: vec!["尺寸 60 × 24 × 16 mm".into(), "材质 PLA".into(), "三道线槽".into(), "售价 29.9 元".into()],
            brand_voice: String::new(),
        }
    }

    const GOOD: &str = r##"{"notes":[
        {"angle":"pain","title":"桌面乱线终结者|磁吸线缆夹","body":"每天坐下来第一眼看到的就是一团线……","tags":["桌搭","3D打印","#线缆收纳"]},
        {"angle":"scene","title":"桌角多了个小东西","body":"白色的小夹子,贴在桌沿。","tags":["桌面收纳"]}
    ]}"##;

    #[tokio::test]
    async fn one_call_writes_one_note_per_angle_in_the_order_we_asked_for() {
        let llm = MockLlm::new([GOOD]);
        let (notes, report) = draft_notes(&llm, &brief(), &[], &PublishConfig::default()).await.unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].angle, notes[1].angle), (NoteAngle::Pain, NoteAngle::Scene), "默认写痛点和场景两篇");
        assert_eq!(notes[0].tags, ["桌搭", "3D打印", "线缆收纳"], "话题里的 # 去掉");
        assert_eq!(report.llm_calls, 1);

        let sent = llm.requests();
        assert!(sent[0].messages[1].content.contains("尺寸 60 × 24 × 16 mm") && sent[0].messages[1].content.contains("pain, scene"));
        assert!(sent[0].messages[1].content.contains("(没有给出)"), "没设品牌语气时如实告诉模型");
    }

    #[tokio::test]
    async fn a_draft_that_breaks_the_rules_is_sent_back_once_with_the_reasons() {
        let bad = r#"{"notes":[{"angle":"pain","title":"全网第一好用的桌面磁吸线缆整理收纳夹子来了","body":"加我微信","tags":[]}]}"#;
        let fixed = r#"{"notes":[{"angle":"pain","title":"磁吸线缆夹","body":"三道线槽。","tags":["桌搭"]}]}"#;
        let llm = MockLlm::new([bad, fixed]);
        let (notes, report) = draft_notes(&llm, &brief(), &[NoteAngle::Pain], &PublishConfig::default()).await.unwrap();
        assert_eq!((notes[0].title.as_str(), report.llm_calls), ("磁吸线缆夹", 2));
        let feedback = llm.requests()[1].messages.last().unwrap().content.clone();
        for needle in ["全网", "微信", "标题不超过 20 字"] {
            assert!(feedback.contains(needle), "重问时要说清楚哪里不行:{feedback}");
        }
    }

    #[tokio::test]
    async fn a_second_imperfect_draft_is_handed_to_the_person_but_a_broken_one_fails() {
        let bad = r#"{"notes":[{"angle":"pain","title":"顶级线缆夹","body":"正文","tags":[]}]}"#;
        let llm = MockLlm::new([bad, bad]);
        let (notes, _) = draft_notes(&llm, &brief(), &[NoteAngle::Pain], &PublishConfig::default()).await.unwrap();
        assert_eq!(notes[0].title, "顶级线缆夹", "第二次还有违禁词:照样交回去,界面上的检查会标出来");

        let short = r#"{"notes":[]}"#;
        let llm = MockLlm::new([short, short]);
        assert!(matches!(draft_notes(&llm, &brief(), &[NoteAngle::Pain], &PublishConfig::default()).await, Err(AgentError::InvalidOutput(_))));
        let llm = MockLlm::new([GOOD]);
        assert!(matches!(draft_notes(&llm, &PublishBrief::default(), &[], &PublishConfig::default()).await, Err(AgentError::EmptyBrief)));
    }

    #[tokio::test]
    async fn listing_copy_is_a_title_a_few_selling_points_and_a_detail_text() {
        let answer = r#"{"title":"磁吸线缆夹 三槽桌面理线器","selling_points":["三道线槽"," 磁吸底座 ",""],"body":"PLA 打印,表面有细微层纹。"}"#;
        let llm = MockLlm::new([answer]);
        let (c, report) = draft_listing(&llm, &brief(), &PublishConfig::default()).await.unwrap();
        assert_eq!(c.selling_points, ["三道线槽", "磁吸底座"]);
        assert_eq!((c.title.as_str(), report.llm_calls), ("磁吸线缆夹 三槽桌面理线器", 1));
    }
}
