//! 大语言模型。

mod mock;
mod openai_compat;

pub use mock::MockLlm;
pub use openai_compat::OpenAiCompat;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// 随消息一起发的图片:`data:image/png;base64,…` 或公开的 https 地址。
    /// 只有支持视觉的模型能用(DeepSeek 目前只有 `deepseek-flash`)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: text.into(),
            images: Vec::new(),
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: text.into(),
            images: Vec::new(),
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: text.into(),
            images: Vec::new(),
        }
    }

    /// 带图的用户消息。
    pub fn user_with_images(text: impl Into<String>, images: Vec<String>) -> Self {
        Self {
            role: Role::User,
            content: text.into(),
            images,
        }
    }
}

/// 图片字节 → data URL。`ext` 是不带点的扩展名;不认识的按 PNG 处理。
pub fn image_data_url(bytes: &[u8], ext: &str) -> String {
    use base64::Engine as _;
    let mime = match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    };
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// 要求输出 JSON 对象。DeepSeek 只有 `json_object` 模式、没有 JSON Schema,
    /// 所以结构校验由调用方做(pp-agent 里反序列化进强类型 + 业务校验 + 失败重试一次)
    pub json: bool,
    /// 思考模式。抽取 / 打分这类步骤关掉它(更快更省),审校步骤再打开
    pub thinking: bool,
    pub max_tokens: Option<u32>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            json: false,
            thinking: false,
            max_tokens: None,
        }
    }

    pub fn json(mut self) -> Self {
        self.json = true;
        self
    }

    pub fn thinking(mut self, on: bool) -> Self {
        self.thinking = on;
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入 token 总数(含缓存命中的部分)
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 输入里命中上下文缓存的部分(便宜一个数量级)
    pub cache_hit_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.cache_hit_tokens += other.cache_hit_tokens;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub content: String,
    /// 思考模式下的思维链(只用于排查,不进报告)
    pub reasoning: Option<String>,
    pub usage: Usage,
}

/// 价格:元 / 百万 token。**是配置不是常量**——出厂预设见 `LlmPricing::deepseek_flash()`,用户可改。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LlmPricing {
    pub input_cache_hit: f64,
    pub input_cache_miss: f64,
    pub output: f64,
}

impl LlmPricing {
    /// DeepSeek-V4.1-Flash 的高峰价(2026-09-20 官方定价页)。空闲时段减半;按高峰价估算,宁多勿少。
    pub fn deepseek_flash() -> Self {
        Self {
            input_cache_hit: 0.04,
            input_cache_miss: 2.0,
            output: 8.0,
        }
    }

    /// DeepSeek-V4-Pro 的高峰价。
    pub fn deepseek_pro() -> Self {
        Self {
            input_cache_hit: 0.30,
            input_cache_miss: 9.0,
            output: 27.0,
        }
    }

    /// 这次调用花了多少「分」(人民币)。
    pub fn cost_fen(&self, u: Usage) -> f64 {
        let miss = u.prompt_tokens.saturating_sub(u.cache_hit_tokens) as f64;
        let yuan = (u.cache_hit_tokens as f64 * self.input_cache_hit
            + miss * self.input_cache_miss
            + u.completion_tokens as f64 * self.output)
            / 1_000_000.0;
        yuan * 100.0
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_separates_cache_hits_from_misses() {
        let p = LlmPricing::deepseek_flash();
        // 100 万输入(其中 40 万命中缓存)+ 10 万输出
        let u = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 100_000,
            cache_hit_tokens: 400_000,
        };
        let yuan = 0.4 * 0.04 + 0.6 * 2.0 + 0.1 * 8.0;
        assert!((p.cost_fen(u) - yuan * 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_typical_research_run_costs_well_under_one_yuan() {
        // docs/04-integrations.md 的估算:约 7.2 万输入 / 1.4 万输出
        let u = Usage {
            prompt_tokens: 72_000,
            completion_tokens: 14_400,
            cache_hit_tokens: 0,
        };
        let fen = LlmPricing::deepseek_flash().cost_fen(u);
        assert!(fen > 20.0 && fen < 30.0, "{fen}");
    }

    #[test]
    fn cache_hits_can_never_make_the_cost_negative() {
        // 供应商返回的命中数大于输入总数(不该发生,但别因此算出负数)
        let u = Usage {
            prompt_tokens: 10,
            completion_tokens: 0,
            cache_hit_tokens: 50,
        };
        assert!(LlmPricing::deepseek_flash().cost_fen(u) >= 0.0);
    }

    #[test]
    fn usage_accumulates() {
        let mut total = Usage::default();
        total.add(Usage {
            prompt_tokens: 10,
            completion_tokens: 2,
            cache_hit_tokens: 4,
        });
        total.add(Usage {
            prompt_tokens: 5,
            completion_tokens: 1,
            cache_hit_tokens: 0,
        });
        assert_eq!(
            total,
            Usage {
                prompt_tokens: 15,
                completion_tokens: 3,
                cache_hit_tokens: 4
            }
        );
    }
}
