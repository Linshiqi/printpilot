//! 第三方供应商的标识与配置状态(设置页展示用)。**密钥本身永远不出现在这些结构里**。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// 调研大脑(LLM)
    Deepseek,
    /// 联网搜索
    ZhipuSearch,
    /// 出图:MiniMax image-01(便宜,只能文生图)
    Minimax,
    /// 出图 + 改图:通义千问图像(阿里云百炼)
    QwenImage,
}

impl ProviderId {
    pub const ALL: [ProviderId; 4] = [ProviderId::Deepseek, ProviderId::ZhipuSearch, ProviderId::Minimax, ProviderId::QwenImage];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Deepseek => "deepseek",
            ProviderId::ZhipuSearch => "zhipu_search",
            ProviderId::Minimax => "minimax",
            ProviderId::QwenImage => "qwen_image",
        }
    }

    /// 去哪里拿密钥(设置页上给用户的链接)。
    pub fn console_url(self) -> &'static str {
        match self {
            ProviderId::Deepseek => "https://platform.deepseek.com/api_keys",
            ProviderId::ZhipuSearch => "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
            ProviderId::Minimax => "https://platform.minimax.cn/user-center/basic-information/interface-key",
            ProviderId::QwenImage => "https://bailian.console.aliyun.com/?tab=model#/api-key",
        }
    }
}

/// 前端只知道「有没有配」,不知道密钥是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub has_key: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_their_wire_format() {
        for id in ProviderId::ALL {
            assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{}\"", id.as_str()));
            assert!(id.console_url().starts_with("https://"));
        }
    }
}
