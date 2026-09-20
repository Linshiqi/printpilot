//! 让模型吐出「能用的」结构化数据。
//!
//! DeepSeek 只有 `json_object` 模式、没有 JSON Schema,而且文档明说偶尔会返回空内容。
//! 所以:反序列化进强类型 → 业务校验 → 不合格就把错误原因告诉模型、再问一次 → 还不行才报错。

use pp_providers::llm::{ChatMessage, ChatRequest, LlmProvider, Usage};
use serde::de::DeserializeOwned;

use crate::AgentError;

/// 从模型的回答里抠出 JSON 对象:容忍 ```json 围栏和前后的客套话。
pub fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

#[derive(Debug)]
pub struct JsonAnswer<T> {
    pub value: T,
    pub usage: Usage,
    pub calls: u32,
    /// 重试了几次(0 或 1)
    pub retries: u32,
}

/// 问一次,要 JSON;不合格则带着错误原因重试一次。`validate` 做业务校验,返回 `Err(原因)` 即不合格。
pub async fn ask_json<T, V>(llm: &dyn LlmProvider, req: ChatRequest, validate: V) -> Result<JsonAnswer<T>, AgentError>
where
    T: DeserializeOwned,
    V: Fn(&T) -> Result<(), String>,
{
    let mut req = req.json();
    let mut usage = Usage::default();
    let mut calls = 0;
    let mut last_problem = String::new();

    for attempt in 0..2u32 {
        calls += 1;
        let answer = match llm.chat(req.clone()).await {
            Ok(a) => a,
            // 空内容之类的坏响应值得再问一次;密钥、余额、限流直接往上抛
            Err(e) if attempt == 0 && matches!(e, pp_providers::ProviderError::BadResponse(_)) => {
                last_problem = e.to_string();
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        usage.add(answer.usage);

        let parsed = extract_json(&answer.content)
            .ok_or_else(|| "输出里没有 JSON 对象".to_string())
            .and_then(|raw| serde_json::from_str::<T>(raw).map_err(|e| format!("JSON 结构不对:{e}")))
            .and_then(|value| validate(&value).map(|()| value));
        match parsed {
            Ok(value) => {
                return Ok(JsonAnswer {
                    value,
                    usage,
                    calls,
                    retries: attempt,
                })
            }
            Err(problem) => {
                log::warn!("[agent] 第 {} 次输出不合格:{problem}", attempt + 1);
                req.messages.push(ChatMessage::assistant(answer.content));
                req.messages.push(ChatMessage::user(format!(
                    "上一次输出不合格:{problem}。请只输出修正后的 json 对象,不要解释。"
                )));
                last_problem = problem;
            }
        }
    }
    Err(AgentError::InvalidOutput(last_problem))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_providers::llm::MockLlm;
    use pp_providers::ProviderError;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Plan {
        queries: Vec<String>,
    }

    fn req() -> ChatRequest {
        ChatRequest::new("m", vec![ChatMessage::user("给我 json")])
    }

    fn non_empty(p: &Plan) -> Result<(), String> {
        if p.queries.is_empty() {
            Err("queries 不能为空".into())
        } else {
            Ok(())
        }
    }

    #[test]
    fn json_is_extracted_from_fences_and_chatter() {
        assert_eq!(extract_json("{\"a\":1}"), Some("{\"a\":1}"));
        assert_eq!(extract_json("好的:\n```json\n{\"a\":{\"b\":2}}\n```\n以上。"), Some("{\"a\":{\"b\":2}}"));
        assert_eq!(extract_json("没有对象"), None);
        assert_eq!(extract_json("} 反了 {"), None);
    }

    #[tokio::test]
    async fn good_output_passes_on_the_first_try_in_json_mode() {
        let llm = MockLlm::new([r#"{"queries":["a","b"]}"#]);
        let ans = ask_json::<Plan, _>(&llm, req(), non_empty).await.unwrap();
        assert_eq!(ans.value.queries, ["a", "b"]);
        assert_eq!((ans.calls, ans.retries), (1, 0));
        assert!(llm.requests()[0].json, "必须以 JSON 模式发请求");
    }

    #[tokio::test]
    async fn malformed_output_is_retried_once_with_the_reason_fed_back() {
        let llm = MockLlm::new(["这不是 JSON", r#"{"queries":["ok"]}"#]);
        let ans = ask_json::<Plan, _>(&llm, req(), non_empty).await.unwrap();
        assert_eq!((ans.calls, ans.retries), (2, 1));
        let second = &llm.requests()[1];
        assert_eq!(second.messages.len(), 3, "原问题 + 上次的坏回答 + 纠正指令");
        assert!(second.messages[2].content.contains("没有 JSON 对象"));
        // 两次调用的用量都要算进去——重试也是花钱的
        assert_eq!(ans.usage.prompt_tokens, 2000);
    }

    #[tokio::test]
    async fn output_that_fails_business_validation_is_also_retried() {
        let llm = MockLlm::new([r#"{"queries":[]}"#, r#"{"queries":["x"]}"#]);
        let ans = ask_json::<Plan, _>(&llm, req(), non_empty).await.unwrap();
        assert_eq!(ans.retries, 1);
        assert!(llm.requests()[1].messages[2].content.contains("queries 不能为空"));
    }

    #[tokio::test]
    async fn two_bad_outputs_in_a_row_give_up_with_the_last_reason() {
        let llm = MockLlm::new(["nope", r#"{"wrong":1}"#]);
        let err = ask_json::<Plan, _>(&llm, req(), non_empty).await.unwrap_err();
        assert!(matches!(err, AgentError::InvalidOutput(ref why) if why.contains("JSON 结构不对")), "{err:?}");
        assert_eq!(llm.calls(), 2, "最多问两次");
    }

    #[tokio::test]
    async fn empty_content_is_retried_but_auth_failures_are_not() {
        let flaky = MockLlm::new(Vec::<String>::new())
            .then_fail(ProviderError::BadResponse("empty content".into()))
            .then_answer(r#"{"queries":["x"]}"#);
        assert!(ask_json::<Plan, _>(&flaky, req(), non_empty).await.is_ok());

        let locked_out = MockLlm::new(Vec::<String>::new()).then_fail(ProviderError::Auth);
        let err = ask_json::<Plan, _>(&locked_out, req(), non_empty).await.unwrap_err();
        assert_eq!(err, AgentError::Provider(ProviderError::Auth));
        assert_eq!(locked_out.calls(), 1, "密钥不对,再问一次也没用");
    }
}
