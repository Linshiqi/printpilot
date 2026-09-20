//! 供应商适配层(docs/03-architecture.md §5)。
//!
//! 约定:
//! - 业务代码只认 trait(`LlmProvider`、`SearchProvider`…),换供应商 = 换一个实现 + 改配置;
//! - **模型名、域名、价格都是配置,不是常量**——调研当天就发现 DeepSeek 换了模型命名、MiniMax 换了域名;
//! - 每个适配器把「拼请求体」和「解析响应」写成纯函数,不联网就能单测;真正发请求的那一层很薄;
//! - 每类供应商都有 mock 实现:演示模式与上层(pp-agent)的单测都用它。

pub mod error;
pub mod http;
pub mod llm;
pub mod search;

pub use error::ProviderError;
