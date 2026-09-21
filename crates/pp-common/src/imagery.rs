//! 图片工作台的共享类型。
//!
//! **画板(board)** = 一个主题的一组图:参考图 + 和 AI 的对话 + 这条线上生成过的每一张图(版本树)。
//! 和建模工作室同一个思路:对话是主线,第一句话出图,之后每句话要么改当前这张图、要么重新生成一批、要么只是回答。

use serde::{Deserialize, Serialize};

/// 这组图拿来做什么——决定提示词的写法和默认比例。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePurpose {
    /// 建模参考图:白底、单个物体、3/4 视角、形体清楚——之后送去建模
    #[default]
    ModelRef,
    /// 场景图:产品放在使用场景里,给笔记 / 详情页配图
    Scene,
    /// 封面:构图留出放标题的空间
    Cover,
    /// 不加任何预设
    Free,
}

impl ImagePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            ImagePurpose::ModelRef => "model_ref",
            ImagePurpose::Scene => "scene",
            ImagePurpose::Cover => "cover",
            ImagePurpose::Free => "free",
        }
    }

    pub fn parse(s: &str) -> ImagePurpose {
        match s {
            "scene" => ImagePurpose::Scene,
            "cover" => ImagePurpose::Cover,
            "free" => ImagePurpose::Free,
            _ => ImagePurpose::ModelRef,
        }
    }

    pub fn default_aspect(self) -> ImageAspect {
        match self {
            ImagePurpose::ModelRef | ImagePurpose::Free => ImageAspect::Square,
            // 小红书笔记与主图推荐 3:4
            ImagePurpose::Scene | ImagePurpose::Cover => ImageAspect::Portrait,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageAspect {
    #[default]
    Square,
    Portrait,
    Landscape,
    Tall,
    Wide,
}

impl ImageAspect {
    pub const ALL: [ImageAspect; 5] = [ImageAspect::Square, ImageAspect::Portrait, ImageAspect::Landscape, ImageAspect::Tall, ImageAspect::Wide];

    pub fn ratio(self) -> &'static str {
        match self {
            ImageAspect::Square => "1:1",
            ImageAspect::Portrait => "3:4",
            ImageAspect::Landscape => "4:3",
            ImageAspect::Tall => "9:16",
            ImageAspect::Wide => "16:9",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ImageAspect::Square => "square",
            ImageAspect::Portrait => "portrait",
            ImageAspect::Landscape => "landscape",
            ImageAspect::Tall => "tall",
            ImageAspect::Wide => "wide",
        }
    }

    pub fn parse(s: &str) -> ImageAspect {
        ImageAspect::ALL.into_iter().find(|a| a.as_str() == s || a.ratio() == s).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageBoard {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub name: String,
    pub purpose: ImagePurpose,
    pub aspect: ImageAspect,
    /// 参考图(用户贴的;资产编号)
    #[serde(default)]
    pub ref_asset_ids: Vec<String>,
    /// 当前选中的那张图(之后说的「改一下」改的就是它)
    #[serde(default)]
    pub current_image_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 画板列表里的一行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageBoardSummary {
    pub id: String,
    pub name: String,
    pub purpose: ImagePurpose,
    /// 封面 = 当前选中的图的资产编号
    #[serde(default)]
    pub cover_asset_id: Option<String>,
    pub images: u32,
    pub updated_at: i64,
}

/// 生成出来的一张图。`parent_id` 有值 = 它是从那张图改出来的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageVersion {
    pub id: String,
    pub board_id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    pub asset_id: String,
    /// 交给图片模型的提示词(编辑时是编辑指令)
    pub prompt: String,
    /// generate(文生图)· edit(按指令改图)
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub aspect: ImageAspect,
    /// 用户采用了这张(之后可以送去建模、当封面)
    pub adopted: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMsgKind {
    /// 文字:用户的话;AI 的回答 / 反问
    Text,
    /// AI 出了一批图
    Images,
}

/// 一轮的花费与过程。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageTurnReport {
    pub llm_calls: u32,
    pub images: u32,
    pub cost_fen: f64,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageMsgExtra {
    /// 这一批图用的提示词
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// generate / edit
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 这一批出的图
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_ids: Vec<String>,
    /// 编辑时:改的是哪一张
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ImageTurnReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageMessage {
    pub id: String,
    pub board_id: String,
    /// user / assistant
    pub from_user: bool,
    pub kind: ImageMsgKind,
    pub content: String,
    #[serde(default)]
    pub extra: ImageMsgExtra,
    /// 用户随这句话贴的图(资产编号)
    #[serde(default)]
    pub image_asset_ids: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageBoardDetail {
    pub board: ImageBoard,
    pub messages: Vec<ImageMessage>,
    /// 最新的在前
    pub versions: Vec<ImageVersion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageTurnResult {
    pub board: ImageBoard,
    pub messages: Vec<ImageMessage>,
    #[serde(default)]
    pub versions: Vec<ImageVersion>,
}

/// 现在能用哪个出图的供应商(设置页 / 工作台顶部展示)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageProviderInfo {
    /// 有可用的供应商(配了密钥,或在演示模式下)
    pub available: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
    /// 能按指令改图(不能的话「改一下」只能改写提示词重新生成,主体会变)
    #[serde(default)]
    pub can_edit: bool,
    #[serde(default)]
    pub price_fen: f64,
    /// 规划用的模型(DeepSeek)也就绪了。每一轮都要先让它看图、写提示词,只配出图的密钥是发不出去的
    #[serde(default)]
    pub planner_ready: bool,
}

/// 出图相关的设置(偏好的供应商、可改的接入地址)。密钥不在这里——密钥在「接口密钥」里、只进系统凭据管理器。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageSettings {
    /// auto / minimax / qwen
    pub provider: String,
    pub minimax_base_url: String,
    pub qwen_base_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purposes_and_aspects_roundtrip_and_have_sensible_defaults() {
        for p in [ImagePurpose::ModelRef, ImagePurpose::Scene, ImagePurpose::Cover, ImagePurpose::Free] {
            assert_eq!(ImagePurpose::parse(p.as_str()), p);
            assert_eq!(serde_json::to_string(&p).unwrap(), format!("\"{}\"", p.as_str()));
        }
        for a in ImageAspect::ALL {
            assert_eq!(ImageAspect::parse(a.as_str()), a);
            assert_eq!(ImageAspect::parse(a.ratio()), a, "也认 3:4 这种写法(模型的回答里会这么写)");
        }
        assert_eq!(ImagePurpose::ModelRef.default_aspect(), ImageAspect::Square);
        assert_eq!(ImagePurpose::Cover.default_aspect().ratio(), "3:4", "小红书封面推荐 3:4");
    }

    #[test]
    fn empty_message_extras_cost_nothing_on_the_wire() {
        assert_eq!(serde_json::to_string(&ImageMsgExtra::default()).unwrap(), "{}");
    }
}
