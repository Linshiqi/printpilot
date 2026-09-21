//! OpenAI 兼容的 `/chat/completions` 客户端。DeepSeek 是出厂预设;任何兼容这套协议的模型
//! (通义、智谱、Kimi、豆包、本地的 Ollama…)换 `base_url` 和模型名即可。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{ChatRequest, ChatResponse, DeltaSink, LlmProvider, StreamDelta, Usage};
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

fn usage_of(v: &Value) -> Usage {
    let num = |path: &str| v.pointer(path).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        prompt_tokens: num("/usage/prompt_tokens"),
        completion_tokens: num("/usage/completion_tokens"),
        // DeepSeek 的字段;OpenAI 风格的放在 prompt_tokens_details.cached_tokens
        cache_hit_tokens: num("/usage/prompt_cache_hit_tokens").max(num("/usage/prompt_tokens_details/cached_tokens")),
    }
}

/// 流式响应(SSE)的解析器:喂字节块进去,吐片段出来;最后 `finish` 得到完整回答。
/// 纯状态机,不碰网络——网络那一层只管把收到的字节原样喂进来。
///
/// 要对付的几件事:一个事件可能被切在两个字节块里(连一个汉字都可能被切开,所以**按字节攒、按整行解**);
/// 以 `:` 开头的是注释(DeepSeek 用它保活);用量在 `[DONE]` 之前单独的一块里;流的中途也可能来一个 `error`。
#[derive(Default)]
pub(crate) struct SseParser {
    pending: Vec<u8>,
    content: String,
    reasoning: String,
    usage: Usage,
    done: bool,
}

impl SseParser {
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<StreamDelta>, ProviderError> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue; // 空行、注释(保活)、event: / id: 之类
            };
            let data = data.trim();
            if data == "[DONE]" {
                self.done = true;
                continue;
            }
            let v: Value = serde_json::from_str(data).map_err(|e| ProviderError::BadResponse(format!("bad stream chunk: {e}")))?;
            if let Some(err) = v.get("error") {
                let msg = err.get("message").and_then(Value::as_str).unwrap_or("stream error");
                return Err(ProviderError::BadResponse(format!("stream error: {msg}")));
            }
            if v.get("usage").is_some_and(|u| !u.is_null()) {
                self.usage = usage_of(&v);
            }
            let delta = v.pointer("/choices/0/delta");
            let text = |key: &str| delta.and_then(|d| d.get(key)).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            if let Some(r) = text("reasoning_content") {
                self.reasoning.push_str(&r);
                out.push(StreamDelta::Reasoning(r));
            }
            if let Some(c) = text("content") {
                self.content.push_str(&c);
                out.push(StreamDelta::Content(c));
            }
        }
        Ok(out)
    }

    pub(crate) fn finish(self) -> Result<ChatResponse, ProviderError> {
        if self.content.trim().is_empty() {
            // 和非流式同一条规则:空内容 = 可重试的坏响应。连 [DONE] 都没等到的,多半是连接中途断了
            let why = if self.done { "empty content" } else { "stream ended before [DONE] with no content" };
            return Err(ProviderError::BadResponse(why.into()));
        }
        Ok(ChatResponse {
            content: self.content,
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            usage: self.usage,
        })
    }
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
    Ok(ChatResponse {
        content,
        reasoning,
        usage: usage_of(v),
    })
}

#[async_trait]
impl LlmProvider for OpenAiCompat {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = build_body(&req, self.send_thinking_field);
        let v = http::post_json(&self.client, &self.url(), &self.api_key, &body).await?;
        parse_response(&v)
    }

    async fn chat_stream(&self, req: ChatRequest, sink: DeltaSink<'_>) -> Result<ChatResponse, ProviderError> {
        let mut body = build_body(&req, self.send_thinking_field);
        body["stream"] = json!(true);
        // 不要这个的话,流式响应里没有用量,花了多少钱就算不出来
        body["stream_options"] = json!({ "include_usage": true });
        let mut resp = self
            .client
            .post(self.url())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.text().await.map_err(|e| ProviderError::Network(e.to_string()))?;
            return Err(ProviderError::from_status(status, &text));
        }
        let mut parser = SseParser::default();
        while let Some(chunk) = resp.chunk().await.map_err(|e| ProviderError::Network(e.to_string()))? {
            for delta in parser.feed(&chunk)? {
                sink(delta);
            }
        }
        parser.finish()
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
    fn the_stream_parser_survives_chunks_cut_anywhere_even_inside_a_character() {
        let stream = concat!(
            ": keep-alive\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"先想想线槽\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"线槽加宽到 \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"8 mm。\"},\"finish_reason\":\"stop\"}],\"usage\":null}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":30,\"prompt_cache_hit_tokens\":100}}\n\n",
            "data: [DONE]\n\n",
        )
        .as_bytes();
        // 不管字节块从哪里切开(包括把一个汉字切成两半),结果都一样
        for size in [1usize, 2, 3, 7, 64, stream.len()] {
            let mut p = SseParser::default();
            let mut deltas = Vec::new();
            for chunk in stream.chunks(size) {
                deltas.extend(p.feed(chunk).unwrap());
            }
            let joined = |want_reasoning: bool| -> String {
                deltas
                    .iter()
                    .filter_map(|d| match d {
                        StreamDelta::Reasoning(t) if want_reasoning => Some(t.as_str()),
                        StreamDelta::Content(t) if !want_reasoning => Some(t.as_str()),
                        _ => None,
                    })
                    .collect()
            };
            assert_eq!(joined(true), "先想想线槽", "块大小 {size}");
            assert_eq!(joined(false), "线槽加宽到 8 mm。", "块大小 {size}");
            let resp = p.finish().unwrap();
            assert_eq!(resp.content, "线槽加宽到 8 mm。");
            assert_eq!(resp.reasoning.as_deref(), Some("先想想线槽"));
            assert_eq!(resp.usage, Usage { prompt_tokens: 120, completion_tokens: 30, cache_hit_tokens: 100 }, "用量在单独的一块里");
        }
    }

    #[test]
    fn a_stream_that_errors_or_dies_early_is_reported_not_swallowed() {
        let mut p = SseParser::default();
        let err = p.feed(b"data: {\"error\":{\"message\":\"overloaded\"}}\n\n").unwrap_err();
        assert!(matches!(err, ProviderError::BadResponse(m) if m.contains("overloaded")));

        // 连接中途断了:什么正文都没收到 → 可重试的坏响应
        let mut p = SseParser::default();
        p.feed(b": keep-alive\n\n").unwrap();
        let err = p.finish().unwrap_err();
        assert!(err.retryable() && matches!(err, ProviderError::BadResponse(m) if m.contains("before [DONE]")));

        // 收到一半断了:已经收到的正文照样交回去(上层的检查会发现代码不完整并走修复)
        let mut p = SseParser::default();
        p.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n").unwrap();
        assert_eq!(p.finish().unwrap().content, "half");

        assert!(SseParser::default().feed(b"data: not json\n").is_err());
    }

    #[test]
    fn streaming_asks_for_usage_and_keeps_every_other_field() {
        let mut body = build_body(&req().thinking(true), true);
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["stream_options"]["include_usage"], true);
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
