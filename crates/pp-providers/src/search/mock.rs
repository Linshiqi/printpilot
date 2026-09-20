use std::sync::Mutex;

use async_trait::async_trait;

use super::{SearchHit, SearchProvider, SearchQuery};
use crate::error::ProviderError;

/// 假搜索:每次检索返回同一组结果(标题里带上检索词,便于断言)。
pub struct MockSearch {
    hits_per_query: usize,
    seen: Mutex<Vec<SearchQuery>>,
    fail_with: Option<ProviderError>,
}

impl MockSearch {
    pub fn new(hits_per_query: usize) -> Self {
        Self {
            hits_per_query,
            seen: Mutex::new(Vec::new()),
            fail_with: None,
        }
    }

    pub fn failing(err: ProviderError) -> Self {
        Self {
            hits_per_query: 0,
            seen: Mutex::new(Vec::new()),
            fail_with: Some(err),
        }
    }

    pub fn queries(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|q| q.query.clone()).collect()
    }
}

#[async_trait]
impl SearchProvider for MockSearch {
    async fn search(&self, q: SearchQuery) -> Result<Vec<SearchHit>, ProviderError> {
        let n = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(q.clone());
            seen.len()
        };
        if let Some(err) = &self.fail_with {
            return Err(err.clone());
        }
        Ok((0..self.hits_per_query)
            .map(|i| SearchHit {
                title: format!("{} · 结果 {}", q.query, i + 1),
                url: format!("https://example.com/q{n}/r{}", i + 1),
                snippet: format!("关于「{}」的一段摘录,第 {} 条。", q.query, i + 1),
                site: "示例站".into(),
                published: Some("2026-08-15".into()),
            })
            .collect())
    }

    fn price_per_query_fen(&self) -> f64 {
        1.0
    }
}
