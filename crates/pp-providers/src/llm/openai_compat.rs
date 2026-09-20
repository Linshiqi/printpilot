//! OpenAI 兼容的 `/chat/completions` 客户端。DeepSeek 是出厂预设;任何兼容这套协议的模型
//! (通义、智谱、Kimi、豆包、本地的 Ollama…)换 `base_url` 和模型名即可。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{ChatRequest, ChatResponse, LlmProvider, Usage};
use crate::error::ProviderError;
use crate::http;

pub struct OpenAiCompat {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    /// 是否发送 DeepSeek 的 `thinking` 字段。对不认识它的兼容实现要关掉,免得被 400 拒绝
    send_thinking_field: bool,
}

impl OpenAiCompat {
    pub const DEEPSEEK_BASE_URL: &'static str = "https://api.deepseek.com";

    pub fn new(client: reqwest::Client, base_url: &str, api_key: &str) -> Result<Self, ProviderError> {
        http::check_base_url(base_url)?;
        if api_key.trim().is_empty() {
            return Err(ProviderError::MissingKey);
        }
        let base = base_url.trim().trim_end_matches('/').to_string();
        Ok(Self {
            client,
            send_thinking_field: base.contains("deepseek"),
            base_url: base,
            api_key: api_key.trim().to_string(),
        })
    }

    pub fn deepseek(client: reqwest::Client, api_key: &str) -> Result<Self, ProviderError> {
        Self::new(client, Self::DEEPSEEK_BASE_URL, api_key)
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

/// 一条消息 → OpenAI 格式。没有图片时 `content` 是字符串;有图片时是内容块数组(文字块 + 图片块)。
fn message_json(m: &super::ChatMessage) -> Value {
    if m.images.is_empty() {
        return json!({ "role": m.role, "content": m.content });
    }
    let mut blocks = vec![json!({ "type": "text", "text": m.content })];
    blocks.extend(
        m.images
            .iter()
            .map(|url| json!({ "type": "image_url", "image_url": { "url": url, "detail": "auto" } })),
    );
    json!({ "role": m.role, "content": blocks })
}

/// 拼请求体(纯函数,可单测)。
pub(crate) fn build_body(req: &ChatRequest, send_thinking_field: bool) -> Value {
    let mut body = json!({
        "model": req.model,
        "messages": req.messages.iter().map(message_json).collect::<Vec<_>>(),
        "stream": false,
    });
    if req.json {
        body["response_format"] = json!({ "type": "json_object" });
    }
    if let Some(n) = req.max_tokens {
        body["max_tokens"] = json!(n);
    }
    if send_thinking_field {
        // DeepSeek V4 默认开着思考;不需要的步骤要显式关掉
        body["thinking"] = json!({ "type": if req.thinking { "enabled" } else { "disabled" } });
    }
    // 不发 temperature:思考模式不支持它,非思考模式用默认值即可
    body
}

/// 解析响应(纯函数,可单测)。
pub(crate) fn parse_response(v: &Value) -> Result<ChatResponse, ProviderError> {
    let message = v
        .pointer("/choices/0/message")
        .ok_or_else(|| ProviderError::BadResponse("no choices[0].message".into()))?;
    let content = message.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    if content.trim().is_empty() {
        // DeepSeek 文档明说 JSON 模式下偶尔会返回空内容 → 当成可重试的坏响应
        return Err(ProviderError::BadResponse("empty content".into()));
    }
    let reasoning = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let num = |path: &str| v.pointer(path).and_then(Value::as_u64).unwrap_or(0);
    Ok(ChatResponse {
        content,
        reasoning,
        usage: Usage {
            prompt_tokens: num("/usage/prompt_tokens"),
            completion_tokens: num("/usage/completion_tokens"),
            // DeepSeek 的字段;OpenAI 风格的放在 prompt_tokens_details.cached_tokens
            cache_hit_tokens: num("/usage/prompt_cache_hit_tokens")
                .max(num("/usage/prompt_tokens_details/cached_tokens")),
        },
    })
}

#[async_trait]
impl LlmProvider for OpenAiCompat {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = build_body(&req, self.send_thinking_field);
        let v = http::post_json(&self.client, &self.url(), &self.api_key, &body).await?;
        parse_response(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    fn req() -> ChatRequest {
        ChatRequest::new("deepseek-flash", vec![ChatMessage::system("s"), ChatMessage::user("u")])
    }

    #[test]
    fn body_has_model_messages_and_no_streaming() {
        let b = build_body(&req(), true);
        assert_eq!(b["model"], "deepseek-flash");
        assert_eq!(b["stream"], false);
        assert_eq!(b["messages"][0], json!({ "role": "system", "content": "s" }));
        assert_eq!(b["messages"][1]["role"], "user");
        assert!(b.get("response_format").is_none());
        assert!(b.get("temperature").is_none(), "思考模式不支持 temperature,干脆不发");
    }

    #[test]
    fn images_turn_the_content_into_text_and_image_blocks() {
        let url = crate::llm::image_data_url(&[0x89, b'P', b'N', b'G'], "png");
        assert_eq!(url, "data:image/png;base64,iVBORw==");
        assert!(crate::llm::image_data_url(b"x", "JPG").starts_with("data:image/jpeg;base64,"));

        let req = ChatRequest::new(
            "deepseek-flash",
            vec![ChatMessage::system("s"), ChatMessage::user_with_images("这是什么零件?", vec![url.clone()])],
        );
        let b = build_body(&req, true);
        assert_eq!(b["messages"][0]["content"], "s", "没有图片的消息仍然是纯字符串");
        let blocks = b["messages"][1]["content"].as_array().expect("带图消息的 content 是数组");
        assert_eq!(blocks[0], json!({ "type": "text", "text": "这是什么零件?" }));
        assert_eq!(blocks[1]["type"], "image_url");
        assert_eq!(blocks[1]["image_url"]["url"], url);
    }

    #[test]
    fn json_mode_thinking_and_max_tokens_are_opt_in() {
        let b = build_body(&req().json().thinking(true).max_tokens(4000), true);
        assert_eq!(b["response_format"], json!({ "type": "json_object" }));
        assert_eq!(b["thinking"], json!({ "type": "enabled" }));
        assert_eq!(b["max_tokens"], 4000);
    }

    #[test]
    fn thinking_is_explicitly_disabled_for_deepseek_but_omitted_for_others() {
        assert_eq!(build_body(&req(), true)["thinking"], json!({ "type": "disabled" }));
        assert!(build_body(&req(), false).get("thinking").is_none());
    }

    #[test]
    fn parses_content_reasoning_and_both_cache_hit_spellings() {
        let deepseek = json!({
            "choices": [{ "message": { "role": "assistant", "content": "{\"a\":1}", "reasoning_content": "想了想" } }],
            "usage": { "prompt_tokens": 120, "completion_tokens": 30, "prompt_cache_hit_tokens": 100, "prompt_cache_miss_tokens": 20 }
        });
        let r = parse_response(&deepseek).unwrap();
        assert_eq!(r.content, "{\"a\":1}");
        assert_eq!(r.reasoning.as_deref(), Some("想了想"));
        assert_eq!(
            r.usage,
            Usage {
                prompt_tokens: 120,
                completion_tokens: 30,
                cache_hit_tokens: 100
            }
        );

        let openai_style = json!({
            "choices": [{ "message": { "content": "hi" } }],
            "usage": { "prompt_tokens": 50, "completion_tokens": 5, "prompt_tokens_details": { "cached_tokens": 40 } }
        });
        assert_eq!(parse_response(&openai_style).unwrap().usage.cache_hit_tokens, 40);
    }

    #[test]
    fn empty_content_is_a_retryable_bad_response() {
        let v = json!({ "choices": [{ "message": { "content": "  " } }], "usage": {} });
        let err = parse_response(&v).unwrap_err();
        assert!(matches!(err, ProviderError::BadResponse(_)));
        assert!(err.retryable());
        assert!(parse_response(&json!({ "error": "x" })).is_err());
    }

    #[test]
    fn constructor_rejects_plain_http_and_blank_keys() {
        let client = reqwest::Client::new();
        assert!(matches!(
            OpenAiCompat::new(client.clone(), "http://api.example.com", "k"),
            Err(ProviderError::BadResponse(_))
        ));
        assert_eq!(
            OpenAiCompat::new(client.clone(), "https://api.deepseek.com", "  ").err(),
            Some(ProviderError::MissingKey)
        );
        let ok = OpenAiCompat::new(client, "https://api.deepseek.com/", " sk-x ").unwrap();
        assert_eq!(ok.url(), "https://api.deepseek.com/chat/completions");
        assert!(ok.send_thinking_field);
    }
}
