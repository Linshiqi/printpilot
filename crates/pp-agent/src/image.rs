//! 图片工作台的「规划」一步:用户说一句话 → 这一轮该做什么(重新生成 / 改当前这张 / 只回答),以及给图片模型的提示词。
//!
//! 图片模型只会画,不会对话;对话的连续性在这一步:它看得见当前选中的那张图、它当初的提示词、
//! 用户新贴的图和之前说过的话。真正调图片接口、落盘、入库在调用方(src-tauri)。
//!
//! 和建模对话同一个原则:**一次模型调用同时完成「判断意图」和「干活(写提示词)」**。

use std::time::Instant;

use pp_common::imagery::{ImageAspect, ImagePurpose};
use pp_providers::llm::{ChatMessage, ChatRequest, LlmPricing, LlmProvider};
use serde::Deserialize;

use crate::chat::ChatLine;
use crate::json::ask_json;
use crate::AgentError;

const PLAN_PROMPT: &str = include_str!("../prompts/image_plan.md");
const MAX_PROMPT_CHARS: usize = 900;
const HISTORY_LINES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageAction {
    Generate,
    Edit,
    Reply,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImagePlan {
    pub action: ImageAction,
    /// 给图片模型的提示词 / 编辑指令(`Reply` 时为空)
    pub prompt: String,
    pub count: u32,
    /// 给用户的一句话
    pub reply: String,
    pub llm_calls: u32,
    pub cost_fen: f64,
    pub elapsed_ms: u64,
}

pub struct ImageTurn<'a> {
    pub history: &'a [ChatLine],
    pub message: &'a str,
    pub purpose: ImagePurpose,
    pub aspect: ImageAspect,
    /// 图片模型能不能按指令改图
    pub can_edit: bool,
    pub max_count: u32,
    /// 用户在输入区选的张数
    pub default_count: u32,
    /// 当前选中的那张图当初的提示词
    pub current_prompt: Option<&'a str>,
    /// 给视觉模型看的图(data URL):有当前图时它排第一,后面是用户这句话里新贴的
    pub images: &'a [String],
    pub has_current: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImagePlanConfig {
    /// 要看图,所以用视觉模型
    pub model: String,
    pub pricing: LlmPricing,
}

impl Default for ImagePlanConfig {
    fn default() -> Self {
        Self {
            model: "deepseek-flash".into(),
            pricing: LlmPricing::deepseek_flash(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    action: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    reply: String,
}

fn purpose_name(p: ImagePurpose) -> &'static str {
    match p {
        ImagePurpose::ModelRef => "建模参考图",
        ImagePurpose::Scene => "场景图",
        ImagePurpose::Cover => "封面",
        ImagePurpose::Free => "自由",
    }
}

fn context_block(turn: &ImageTurn<'_>) -> String {
    let attached = turn.images.len().saturating_sub(turn.has_current as usize);
    let mut out = format!(
        "## 这一轮的情况\n- 用途:{}\n- 画幅:{}\n- 图片模型能按指令编辑已有的图:{}\n- 默认张数:{};上限:{}\n",
        purpose_name(turn.purpose),
        turn.aspect.ratio(),
        if turn.can_edit { "能" } else { "不能(只能从文字重新生成)" },
        turn.default_count.clamp(1, turn.max_count),
        turn.max_count,
    );
    match (turn.has_current, turn.current_prompt) {
        (true, Some(p)) => out.push_str(&format!("- 当前选中的图:第 1 张图片。它当初的提示词:{}\n", p.trim())),
        // 没有提示词 = 这张图是用户自己导入的(实拍、草图、别处找的参考),不是模型画的
        (true, None) => out.push_str("- 当前选中的图:第 1 张图片。它是用户自己导入的,不是模型生成的,所以没有提示词——要重新生成的话,提示词得照着画面从头写。\n"),
        (false, _) => out.push_str("- 当前选中的图:还没有(这是第一次出图,或者用户没有选中任何一张)。\n"),
    }
    if attached > 0 {
        out.push_str(&format!("- 用户这句话里新贴了 {attached} 张图(排在{}):是参考,或者是要改的图。\n", if turn.has_current { "当前图之后" } else { "最前面" }));
    }
    out.push_str(&format!("\n## 用户这句话\n{}\n", turn.message.trim()));
    out
}

fn validate(raw: &RawPlan, turn: &ImageTurn<'_>) -> Result<(), String> {
    match raw.action.as_str() {
        "reply" => {
            if raw.reply.trim().is_empty() {
                return Err("action 为 reply 时 reply 不能为空".into());
            }
        }
        "generate" | "edit" => {
            if raw.prompt.trim().is_empty() {
                return Err("出图时 prompt 不能为空".into());
            }
            if raw.action == "edit" && !turn.can_edit {
                return Err("这个图片模型不能编辑已有的图:请改用 generate,把修改要求写进一份完整的新提示词,并在 reply 里说明主体会有变化".into());
            }
            if raw.action == "edit" && turn.images.is_empty() {
                return Err("没有可以编辑的图(没有选中的图,用户也没有贴图):请改用 generate".into());
            }
        }
        other => return Err(format!("action 只能是 generate / edit / reply,现在是 {other}")),
    }
    Ok(())
}

/// 规划这一轮。
pub async fn plan_image_turn(llm: &dyn LlmProvider, turn: ImageTurn<'_>, cfg: &ImagePlanConfig) -> Result<ImagePlan, AgentError> {
    if turn.message.trim().is_empty() && turn.images.is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    let mut messages = vec![ChatMessage::system(PLAN_PROMPT)];
    // 历史只给文字、只留最近几句,而且要从用户的话开始
    let recent: Vec<&ChatLine> = turn.history.iter().rev().take(HISTORY_LINES).collect::<Vec<_>>().into_iter().rev().collect();
    let start = recent.iter().position(|l| l.from_user).unwrap_or(recent.len());
    for line in &recent[start..] {
        let text: String = line.text.chars().take(400).collect();
        messages.push(if line.from_user { ChatMessage::user(text) } else { ChatMessage::assistant(text) });
    }
    messages.push(ChatMessage::user_with_images(context_block(&turn), turn.images.to_vec()));

    let req = ChatRequest::new(&cfg.model, messages).thinking(false).max_tokens(1500);
    let answer = ask_json::<RawPlan, _>(llm, req, |raw| validate(raw, &turn)).await?;
    let raw = answer.value;
    let action = match raw.action.as_str() {
        "generate" => ImageAction::Generate,
        "edit" => ImageAction::Edit,
        _ => ImageAction::Reply,
    };
    let default_count = if action == ImageAction::Edit { turn.default_count.min(2) } else { turn.default_count };
    Ok(ImagePlan {
        action,
        prompt: raw.prompt.trim().chars().take(MAX_PROMPT_CHARS).collect(),
        count: raw.count.unwrap_or(default_count).clamp(1, turn.max_count.max(1)),
        reply: raw.reply.trim().to_string(),
        llm_calls: answer.calls,
        cost_fen: cfg.pricing.cost_fen(answer.usage),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_providers::llm::MockLlm;

    fn turn<'a>(message: &'a str, images: &'a [String], has_current: bool, can_edit: bool) -> ImageTurn<'a> {
        ImageTurn {
            history: &[],
            message,
            purpose: ImagePurpose::ModelRef,
            aspect: ImageAspect::Square,
            can_edit,
            max_count: 6,
            default_count: 4,
            current_prompt: has_current.then_some("白底,单个线缆夹,3/4 视角"),
            images,
            has_current,
        }
    }

    #[tokio::test]
    async fn the_first_message_becomes_a_generation_with_a_written_prompt() {
        let llm = MockLlm::new([r#"{"action":"generate","prompt":"纯白无缝背景,单个桌面线缆夹,3/4 俯视","count":4,"reply":"按白底 3/4 视角出 4 张。"}"#]);
        let plan = plan_image_turn(&llm, turn("做一个桌面线缆夹的参考图", &[], false, true), &ImagePlanConfig::default())
            .await
            .unwrap();
        assert_eq!((plan.action, plan.count), (ImageAction::Generate, 4));
        assert!(plan.prompt.contains("线缆夹") && plan.reply.contains("4 张"));
        assert!(plan.cost_fen > 0.0);

        let req = &llm.requests()[0];
        assert_eq!(req.model, "deepseek-flash");
        assert!(req.json && !req.thinking);
        assert!(req.messages[0].content.contains("json"), "DeepSeek 的 JSON 模式要求提示词里出现 json");
        let ctx = &req.messages.last().unwrap().content;
        assert!(ctx.contains("建模参考图") && ctx.contains("1:1") && ctx.contains("还没有") && ctx.contains("默认张数:4"));
    }

    #[tokio::test]
    async fn an_edit_sees_the_current_image_and_its_prompt() {
        let images = vec!["data:image/jpeg;base64,CURRENT".to_string(), "data:image/jpeg;base64,PASTED".to_string()];
        let history = [
            ChatLine { from_user: false, text: "(孤零零的一句 AI 的话,应当被丢掉)".into() },
            ChatLine { from_user: true, text: "做一个线缆夹".into() },
            ChatLine { from_user: false, text: "出了 4 张。".into() },
        ];
        let llm = MockLlm::new([r#"{"action":"edit","prompt":"把背景换成浅色木纹桌面,主体保持不变","reply":"只换背景,夹子本身不动。"}"#]);
        let mut t = turn("背景换成木桌面,像我贴的这张", &images, true, true);
        t.history = &history;
        let plan = plan_image_turn(&llm, t, &ImagePlanConfig::default()).await.unwrap();
        assert_eq!((plan.action, plan.count), (ImageAction::Edit, 2), "编辑默认最多 2 张");

        let req = &llm.requests()[0];
        assert_eq!(req.messages.len(), 4, "系统 + 两句历史(从用户的话开始)+ 这一句");
        assert_eq!(req.messages[1].content, "做一个线缆夹");
        let last = req.messages.last().unwrap();
        assert_eq!(last.images, images, "当前图在前,新贴的图在后");
        assert!(last.content.contains("它当初的提示词:白底,单个线缆夹") && last.content.contains("新贴了 1 张图"));
    }

    #[tokio::test]
    async fn an_imported_picture_is_introduced_as_the_users_own() {
        let images = vec!["data:image/jpeg;base64,PHOTO".to_string()];
        let llm = MockLlm::new([r#"{"action":"edit","prompt":"把背景换成纯白,主体保持不变","reply":"只换背景。"}"#]);
        let mut t = turn("背景换成纯白", &images, true, true);
        t.current_prompt = None;
        plan_image_turn(&llm, t, &ImagePlanConfig::default()).await.unwrap();
        let requests = llm.requests();
        let ctx = &requests[0].messages.last().unwrap().content;
        assert!(ctx.contains("用户自己导入的") && !ctx.contains("它当初的提示词"), "导入的图没有提示词,要明说:{ctx}");
    }

    #[tokio::test]
    async fn a_generate_only_provider_is_never_asked_to_edit() {
        let images = vec!["data:image/jpeg;base64,CURRENT".to_string()];
        let llm = MockLlm::new([
            r#"{"action":"edit","prompt":"背景换成木纹","reply":"换背景。"}"#,
            r#"{"action":"generate","prompt":"浅色木纹桌面上的单个线缆夹,3/4 视角,自然光","count":2,"reply":"这个模型不能直接改图,我按新的描述重新生成了,主体会有变化。"}"#,
        ]);
        let plan = plan_image_turn(&llm, turn("背景换成木桌面", &images, true, false), &ImagePlanConfig::default())
            .await
            .unwrap();
        assert_eq!(plan.action, ImageAction::Generate);
        assert_eq!(plan.llm_calls, 2);
        assert!(plan.reply.contains("不能直接改图"));
        assert!(llm.requests()[1].messages.last().unwrap().content.contains("不能编辑已有的图"), "打回时要说清为什么");
        assert!(llm.requests()[0].messages.last().unwrap().content.contains("不能(只能从文字重新生成)"));
    }

    #[tokio::test]
    async fn questions_get_answers_and_counts_are_clamped() {
        let llm = MockLlm::new([r#"{"action":"reply","prompt":"","reply":"第二张更适合当主图:主体完整、背景干净。"}"#]);
        let plan = plan_image_turn(&llm, turn("哪张更适合当主图?", &[], false, true), &ImagePlanConfig::default()).await.unwrap();
        assert_eq!(plan.action, ImageAction::Reply);
        assert!(plan.prompt.is_empty());

        let llm = MockLlm::new([r#"{"action":"generate","prompt":"白底线缆夹","count":40,"reply":"出图。"}"#]);
        let plan = plan_image_turn(&llm, turn("来 40 张", &[], false, true), &ImagePlanConfig::default()).await.unwrap();
        assert_eq!(plan.count, 6, "不超过供应商的上限");
    }

    #[tokio::test]
    async fn an_edit_with_nothing_to_edit_is_sent_back() {
        let llm = MockLlm::new([
            r#"{"action":"edit","prompt":"换背景","reply":"换。"}"#,
            r#"{"action":"generate","prompt":"木纹桌面上的线缆夹","reply":"没有可改的图,重新出一批。"}"#,
        ]);
        let plan = plan_image_turn(&llm, turn("背景换一下", &[], false, true), &ImagePlanConfig::default()).await.unwrap();
        assert_eq!(plan.action, ImageAction::Generate);
        assert!(llm.requests()[1].messages.last().unwrap().content.contains("没有可以编辑的图"));

        let nothing = MockLlm::new(Vec::<String>::new());
        let err = plan_image_turn(&nothing, turn("  ", &[], false, true), &ImagePlanConfig::default()).await.unwrap_err();
        assert_eq!(err, AgentError::EmptyBrief);
    }
}
