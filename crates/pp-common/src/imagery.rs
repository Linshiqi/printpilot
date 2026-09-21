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

    fn value(self) -> f64 {
        match self {
            ImageAspect::Square => 1.0,
            ImageAspect::Portrait => 3.0 / 4.0,
            ImageAspect::Landscape => 4.0 / 3.0,
            ImageAspect::Tall => 9.0 / 16.0,
            ImageAspect::Wide => 16.0 / 9.0,
        }
    }

    /// 一张现成的图(用户导入的)最接近哪种画幅。按比例的对数距离比——2:1 和 1:2 离 1:1 一样远。
    pub fn nearest(width: u32, height: u32) -> ImageAspect {
        if width == 0 || height == 0 {
            return ImageAspect::default();
        }
        let r = (width as f64 / height as f64).ln();
        ImageAspect::ALL
            .into_iter()
            .min_by(|a, b| (a.value().ln() - r).abs().total_cmp(&(b.value().ln() - r).abs()))
            .unwrap_or_default()
    }
}

/// 能导入的图片格式(和后端解码器开的格式一致)。HEIC / AVIF 解不了:要先另存为 JPG。
pub const IMPORT_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "jfif", "webp"];
/// 一次最多导入几张——防止把整个相册拖进来
pub const IMPORT_MAX_FILES: usize = 20;

/// 按扩展名看是不是能导入的图片(拖进来的东西先过这一道,真正认不认得还要看解码)。
pub fn is_importable_image(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && IMPORT_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
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

/// 画板上的一张图:模型生成的,或者用户自己导入的。`parent_id` 有值 = 它是从那张图改出来的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageVersion {
    pub id: String,
    pub board_id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    pub asset_id: String,
    /// 交给图片模型的提示词(编辑时是编辑指令);导入的图没有,是空的
    pub prompt: String,
    /// generate(文生图)· edit(按指令改图)· import(用户自己导入的:本地文件、拖进来的、粘贴的)
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
    /// 一批图:AI 出的(`from_user = false`),或者用户导入的(`from_user = true`,`extra.mode = import`)
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
    /// generate / edit / import
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 这一批出的(或导入的)图
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

/// 没导入成的一个文件:文件名 + 原因(`#错误码#细节`,前端按码翻成当前语言)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportFailure {
    pub name: String,
    pub reason: String,
}

/// 导入一批图片的结果。能导入的照常导入,导不进来的逐个说明——不因为一张坏图就整批作废。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageImportResult {
    pub board: ImageBoard,
    /// 时间线上记的那一条「导入了这几张」;一张都没导入成就没有
    #[serde(default)]
    pub message: Option<ImageMessage>,
    /// 新增的图,第一个文件在前(它成为当前选中的图)
    #[serde(default)]
    pub versions: Vec<ImageVersion>,
    #[serde(default)]
    pub failed: Vec<ImportFailure>,
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
    fn an_imported_picture_gets_the_closest_aspect() {
        assert_eq!(ImageAspect::nearest(1000, 1000), ImageAspect::Square);
        assert_eq!(ImageAspect::nearest(3024, 4032), ImageAspect::Portrait, "手机竖拍");
        assert_eq!(ImageAspect::nearest(4032, 3024), ImageAspect::Landscape);
        assert_eq!(ImageAspect::nearest(1080, 1920), ImageAspect::Tall);
        assert_eq!(ImageAspect::nearest(2560, 1440), ImageAspect::Wide, "屏幕截图");
        assert_eq!(ImageAspect::nearest(1080, 1440), ImageAspect::Portrait);
        assert_eq!(ImageAspect::nearest(5000, 1000), ImageAspect::Wide, "比 16:9 还扁的归到最扁的那一档");
        assert_eq!(ImageAspect::nearest(0, 10), ImageAspect::Square, "坏尺寸不 panic");
    }

    #[test]
    fn only_files_with_a_supported_picture_extension_are_importable() {
        for ok in ["C:\\图\\线缆夹.JPG", "/home/u/a.b/ref.webp", "shot.png", "x.jpeg", "saved-from-browser.jfif"] {
            assert!(is_importable_image(ok), "{ok}");
        }
        for no in ["C:\\图\\IMG_0001.HEIC", "model.stl", "noext", "C:\\folder.png\\readme", ".png", "a.avif", ""] {
            assert!(!is_importable_image(no), "{no}");
        }
    }

    #[test]
    fn empty_message_extras_cost_nothing_on_the_wire() {
        assert_eq!(serde_json::to_string(&ImageMsgExtra::default()).unwrap(), "{}");
    }
}
