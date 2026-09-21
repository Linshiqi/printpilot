//! 建模工作室的共享类型。
//!
//! **设计(design)** = 一个零件的一条建模线索:参考图、设计规格、和 AI 的对话、版本树。
//! 对话是主线:第一句话出规格,确认后生成;之后每句话要么改模型(出一个新版本)、要么回答问题、要么反问澄清。
//! 改参数、手改代码、看图复核也都记在同一条时间线上——回头看得出这个零件是怎么一步步改成现在这样的。

use serde::{Deserialize, Serialize};

use crate::cad::{CadBuildReport, CadPick, CadReview, CadVersion, DesignSpec};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadDesign {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub name: String,
    /// 当前认可的设计规格(生成之前给人审的那份;对话里可以反复改)
    #[serde(default)]
    pub spec: Option<DesignSpec>,
    /// 这个设计的参考图(资产编号)
    #[serde(default)]
    pub ref_asset_ids: Vec<String>,
    /// 视图里选中的版本。对着旧版本说话 = 从那一版分叉
    #[serde(default)]
    pub current_version_id: Option<String>,
    /// 列表里的小缩略图(JPEG data URL,几 KB)
    #[serde(default)]
    pub thumb: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 设计列表里的一行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadDesignSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
    /// 当前版本的包围盒(还没有模型时为空)
    #[serde(default)]
    pub size: Option<[f64; 3]>,
    pub versions: u32,
    pub updated_at: i64,
}

/// 谁说的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgRole {
    User,
    Assistant,
    /// 不是对话,是时间线上的一件事:改了参数、手改代码运行了
    Event,
}

/// 这条消息是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgKind {
    /// 普通文字(用户的话;AI 的回答 / 反问)
    Text,
    /// AI 给出的设计规格卡(等用户确认或继续改)
    Spec,
    /// 模型改好了:带着新版本
    Build,
    /// 改了但一版能跑的都没有:带着最后一版代码和每一轮的报错
    Failed,
    /// 看图复核的结论
    Review,
    /// 事件:改参数
    Param,
    /// 事件:手改代码运行
    Manual,
}

impl MsgRole {
    pub fn as_str(self) -> &'static str {
        match self {
            MsgRole::User => "user",
            MsgRole::Assistant => "assistant",
            MsgRole::Event => "event",
        }
    }

    pub fn parse(s: &str) -> MsgRole {
        match s {
            "user" => MsgRole::User,
            "assistant" => MsgRole::Assistant,
            _ => MsgRole::Event,
        }
    }
}

impl MsgKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MsgKind::Text => "text",
            MsgKind::Spec => "spec",
            MsgKind::Build => "build",
            MsgKind::Failed => "failed",
            MsgKind::Review => "review",
            MsgKind::Param => "param",
            MsgKind::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> MsgKind {
        match s {
            "spec" => MsgKind::Spec,
            "build" => MsgKind::Build,
            "failed" => MsgKind::Failed,
            "review" => MsgKind::Review,
            "param" => MsgKind::Param,
            "manual" => MsgKind::Manual,
            _ => MsgKind::Text,
        }
    }
}

/// 消息里随类型而定的部分(库里存成一列 JSON)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MsgExtra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<DesignSpec>,
    /// 花费与过程(AI 的每条消息都有;`build` / `failed` 里还有修复过程与改动的段)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<CadBuildReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<CadReview>,
    /// 用户说这句话时在模型上点中的位置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<CadPick>,
    /// `failed`:最后一版代码(用户可以拿去手改)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 这句话是对着哪个版本说的 / 这次修改改自哪个版本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadMessage {
    pub id: String,
    pub design_id: String,
    pub role: MsgRole,
    pub kind: MsgKind,
    pub content: String,
    #[serde(default)]
    pub extra: MsgExtra,
    /// 用户随这句话贴的图(资产编号)
    #[serde(default)]
    pub image_asset_ids: Vec<String>,
    /// 这条消息产出的版本(`build` / `param` / `manual`)
    #[serde(default)]
    pub version_id: Option<String>,
    pub created_at: i64,
}

/// 打开一个设计时一次拿全。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadDesignDetail {
    pub design: CadDesign,
    pub messages: Vec<CadMessage>,
    /// 最新的在前
    pub versions: Vec<CadVersion>,
}

/// 任何「往时间线上加东西」的操作的结果:新增的消息、(可能有的)新版本、更新后的设计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadTurnResult {
    pub design: CadDesign,
    pub messages: Vec<CadMessage>,
    #[serde(default)]
    pub version: Option<CadVersion>,
}

/// 这一句用哪一档模型。创建用精细;微调时快慢由用户自己权衡。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CadTier {
    /// 快:便宜的模型、不思考。改个尺寸、加个倒角够用
    Fast,
    /// 均衡:最强的模型、不思考
    #[default]
    Balanced,
    /// 精细:最强的模型 + 思考。慢,但复杂改动更稳
    Precise,
}

impl CadTier {
    pub fn as_str(self) -> &'static str {
        match self {
            CadTier::Fast => "fast",
            CadTier::Balanced => "balanced",
            CadTier::Precise => "precise",
        }
    }

    pub fn parse(s: &str) -> CadTier {
        match s {
            "fast" => CadTier::Fast,
            "precise" => CadTier::Precise,
            _ => CadTier::Balanced,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_kinds_and_tiers_roundtrip_through_their_wire_names() {
        for r in [MsgRole::User, MsgRole::Assistant, MsgRole::Event] {
            assert_eq!(MsgRole::parse(r.as_str()), r);
            assert_eq!(serde_json::to_string(&r).unwrap(), format!("\"{}\"", r.as_str()));
        }
        for k in [MsgKind::Text, MsgKind::Spec, MsgKind::Build, MsgKind::Failed, MsgKind::Review, MsgKind::Param, MsgKind::Manual] {
            assert_eq!(MsgKind::parse(k.as_str()), k);
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.as_str()));
        }
        for t in [CadTier::Fast, CadTier::Balanced, CadTier::Precise] {
            assert_eq!(CadTier::parse(t.as_str()), t);
        }
        assert_eq!(CadTier::parse("whatever"), CadTier::Balanced, "不认识的档位按默认");
    }

    #[test]
    fn empty_extras_cost_nothing_on_the_wire() {
        assert_eq!(serde_json::to_string(&MsgExtra::default()).unwrap(), "{}");
        let back: MsgExtra = serde_json::from_str("{}").unwrap();
        assert_eq!(back, MsgExtra::default());
    }
}
