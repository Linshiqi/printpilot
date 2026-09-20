use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Model3d,
    Doc,
    Photo,
    Video,
    Pack,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AssetKind::Image => "image",
            AssetKind::Model3d => "model3d",
            AssetKind::Doc => "doc",
            AssetKind::Photo => "photo",
            AssetKind::Video => "video",
            AssetKind::Pack => "pack",
        }
    }

    pub fn parse(s: &str) -> Option<AssetKind> {
        [
            AssetKind::Image,
            AssetKind::Model3d,
            AssetKind::Doc,
            AssetKind::Photo,
            AssetKind::Video,
            AssetKind::Pack,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// 资产:项目名下的一个文件(图片、3D 模型、发布包…)。
/// `parent_asset_id` 记录血缘:参考图 → 原始模型 → 编辑后的模型 → 打印文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub kind: AssetKind,
    /// 用途:concept_ref / concept_scene / cover / real_photo / model_raw / model_edited / print_file …
    pub role: String,
    /// 相对资产库根目录的路径(正斜杠)。**前端永远拿不到绝对路径**,读文件走 pp-asset://<id>
    pub rel_path: String,
    /// 小写扩展名,不带点:glb / stl / png …
    pub ext: String,
    pub bytes: i64,
    #[serde(default)]
    pub meta_json: Option<String>,
    #[serde(default)]
    pub parent_asset_id: Option<String>,
    #[serde(default)]
    pub source_job_id: Option<String>,
    /// AI 生成:驱动 AIGC 标识与发布时的声明提醒
    pub ai_generated: bool,
    pub is_adopted: bool,
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_str_roundtrip_matches_serde() {
        for k in [
            AssetKind::Image,
            AssetKind::Model3d,
            AssetKind::Doc,
            AssetKind::Photo,
            AssetKind::Video,
            AssetKind::Pack,
        ] {
            assert_eq!(AssetKind::parse(k.as_str()), Some(k));
            assert_eq!(
                serde_json::to_string(&k).unwrap(),
                format!("\"{}\"", k.as_str())
            );
        }
    }
}
