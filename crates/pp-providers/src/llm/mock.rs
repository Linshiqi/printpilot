use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use super::{ChatRequest, ChatResponse, LlmProvider, Usage};
use crate::error::ProviderError;

/// 按脚本回答的假模型:上层流水线的单测与演示模式都用它。
/// 每次 `chat` 依次弹出一条预置的回答;脚本用完了就报错(测试因此能发现「多问了一次」)。
pub struct MockLlm {
    script: Mutex<VecDeque<Result<String, ProviderError>>>,
    seen: Mutex<Vec<ChatRequest>>,
    usage_per_call: Usage,
}

impl MockLlm {
    pub fn new<I, S>(answers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            script: Mutex::new(answers.into_iter().map(|s| Ok(s.into())).collect()),
            seen: Mutex::new(Vec::new()),
            usage_per_call: Usage {
                prompt_tokens: 1000,
                completion_tokens: 200,
                cache_hit_tokens: 0,
            },
        }
    }

    /// 在脚本末尾追加一次失败(测重试与错误传播)。
    pub fn then_fail(self, err: ProviderError) -> Self {
        self.script.lock().unwrap().push_back(Err(err));
        self
    }

    pub fn then_answer(self, text: impl Into<String>) -> Self {
        self.script.lock().unwrap().push_back(Ok(text.into()));
        self
    }

    /// 到目前为止收到的全部请求(断言提示词里带了什么)。
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }

    pub fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmProvider for MockLlm {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.seen.lock().unwrap().push(req);
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(ProviderError::BadResponse("mock script exhausted".into())));
        next.map(|content| ChatResponse {
            content,
            reasoning: None,
            usage: self.usage_per_call,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    #[tokio::test]
    async fn answers_in_order_records_requests_then_runs_dry() {
        let llm = MockLlm::new(["one"]).then_fail(ProviderError::RateLimited).then_answer("three");
        let req = || ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        assert_eq!(llm.chat(req()).await.unwrap().content, "one");
        assert_eq!(llm.chat(req()).await.unwrap_err(), ProviderError::RateLimited);
        assert_eq!(llm.chat(req()).await.unwrap().content, "three");
        assert!(llm.chat(req()).await.is_err(), "脚本用完要报错,不能悄悄给空回答");
        assert_eq!(llm.calls(), 4);
        assert_eq!(llm.requests()[0].messages[0].content, "hi");
    }
}
