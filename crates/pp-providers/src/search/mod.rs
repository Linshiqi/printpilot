//! 联网搜索。DeepSeek 的 API 不内置搜索,调研 Agent 的检索靠这一层。
//!
//! 已知边界(docs/04-integrations.md §2):这些搜索接口都**搜不到小红书站内内容**(robots 限制),
//! 能搜到的是媒体、论坛、电商公开页。平台内的一手观察要靠用户补充证据。

mod mock;
mod zhipu;

pub use mock::MockSearch;
pub use zhipu::ZhipuSearch;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Recency {
    Week,
    Month,
    Year,
    #[default]
    Any,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchQuery {
    pub query: String,
    pub count: u32,
    pub recency: Recency,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            count: 8,
            recency: Recency::Year,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    /// 正文摘录(供应商给多长就多长;流水线里再截断)
    pub snippet: String,
    /// 来源站点名
    pub site: String,
    /// 发布日期,`YYYY-MM-DD`;不少结果没有
    pub published: Option<String>,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, q: SearchQuery) -> Result<Vec<SearchHit>, ProviderError>;

    /// 每次检索的价格(分)。是配置不是常量,这里给的是出厂预设。
    fn price_per_query_fen(&self) -> f64;
}
