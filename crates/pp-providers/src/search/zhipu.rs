//! 智谱 Web Search(独立搜索接口)。出厂默认的搜索供应商:`search_std` 每次 0.01 元,最便宜。
//! 文档:https://docs.bigmodel.cn/cn/guide/tools/web-search

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Recency, SearchHit, SearchProvider, SearchQuery};
use crate::error::ProviderError;
use crate::http;

pub struct ZhipuSearch {
    client: reqwest::Client,
    api_key: String,
    endpoint: String,
    engine: String,
}

impl ZhipuSearch {
    pub const ENDPOINT: &'static str = "https://open.bigmodel.cn/api/paas/v4/web_search";
    /// 接口限制:检索词最多 70 个字符
    const MAX_QUERY_CHARS: usize = 70;

    pub fn new(client: reqwest::Client, api_key: &str) -> Result<Self, ProviderError> {
        if api_key.trim().is_empty() {
            return Err(ProviderError::MissingKey);
        }
        Ok(Self {
            client,
            api_key: api_key.trim().to_string(),
            endpoint: Self::ENDPOINT.to_string(),
            engine: "search_std".to_string(),
        })
    }
}

pub(crate) fn build_body(q: &SearchQuery, engine: &str) -> Value {
    // 按字符截断(不是按字节),否则会把汉字切成两半
    let query: String = q.query.trim().chars().take(ZhipuSearch::MAX_QUERY_CHARS).collect();
    json!({
        "search_query": query,
        "search_engine": engine,
        "search_intent": false,
        "count": q.count.clamp(1, 50),
        "search_recency_filter": match q.recency {
            Recency::Week => "oneWeek",
            Recency::Month => "oneMonth",
            Recency::Year => "oneYear",
            Recency::Any => "noLimit",
        },
        // high = 返回更长的正文摘录:我们直接拿它当证据,不再单独抓网页
        "content_size": "high",
    })
}

pub(crate) fn parse_response(v: &Value) -> Result<Vec<SearchHit>, ProviderError> {
    let items = v
        .get("search_result")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::BadResponse("no search_result".into()))?;
    let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    Ok(items
        .iter()
        .map(|item| SearchHit {
            title: text(item, "title"),
            url: text(item, "link"),
            snippet: text(item, "content"),
            site: text(item, "media"),
            published: Some(text(item, "publish_date")).filter(|d| !d.is_empty()),
        })
        // 没有链接的结果没法作为证据引用
        .filter(|hit| !hit.url.is_empty())
        .collect())
}

#[async_trait]
impl SearchProvider for ZhipuSearch {
    async fn search(&self, q: SearchQuery) -> Result<Vec<SearchHit>, ProviderError> {
        let body = build_body(&q, &self.engine);
        let v = http::post_json(&self.client, &self.endpoint, &self.api_key, &body).await?;
        parse_response(&v)
    }

    fn price_per_query_fen(&self) -> f64 {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_maps_recency_and_clamps_count() {
        let mut q = SearchQuery::new("3D打印 桌面收纳");
        q.count = 500;
        q.recency = Recency::Month;
        let b = build_body(&q, "search_std");
        assert_eq!(b["search_query"], "3D打印 桌面收纳");
        assert_eq!(b["search_engine"], "search_std");
        assert_eq!(b["search_intent"], false);
        assert_eq!(b["count"], 50);
        assert_eq!(b["search_recency_filter"], "oneMonth");
        assert_eq!(b["content_size"], "high");
    }

    #[test]
    fn long_queries_are_cut_at_70_characters_not_bytes() {
        let q = SearchQuery::new("打".repeat(100));
        let cut = build_body(&q, "search_std")["search_query"].as_str().unwrap().to_string();
        assert_eq!(cut.chars().count(), 70);
    }

    #[test]
    fn response_items_become_hits_and_linkless_ones_are_dropped() {
        let v = json!({
            "search_result": [
                { "title": " 桌搭好物 ", "link": "https://a.example/1", "content": "正文…", "media": "某站", "publish_date": "2026-08-01" },
                { "title": "没日期", "link": "https://a.example/2", "content": "x", "media": "", "publish_date": "" },
                { "title": "没链接", "link": "", "content": "x" }
            ]
        });
        let hits = parse_response(&v).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "桌搭好物");
        assert_eq!(hits[0].published.as_deref(), Some("2026-08-01"));
        assert_eq!(hits[1].published, None);
    }

    #[test]
    fn a_response_without_results_is_an_error_not_an_empty_list() {
        assert!(parse_response(&json!({ "error": { "message": "bad key" } })).is_err());
        assert_eq!(parse_response(&json!({ "search_result": [] })).unwrap(), vec![]);
    }
}
