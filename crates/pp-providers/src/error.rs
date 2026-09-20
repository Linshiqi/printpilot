/// 供应商调用失败的原因。上层据此决定:提示用户去配密钥、充值、稍后重试,还是直接报错。
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderError {
    /// 这类能力还没配密钥 → 界面走「能力降级」,而不是弹错误
    MissingKey,
    /// 401 / 403:密钥不对或没权限
    Auth,
    /// 402:余额不足
    InsufficientBalance,
    /// 429:被限流
    RateLimited,
    /// 其它非 2xx
    Http { status: u16, body: String },
    /// 连不上、超时、TLS 失败
    Network(String),
    /// 2xx 但内容不是我们能用的(结构不对、内容为空)
    BadResponse(String),
}

impl ProviderError {
    /// 对应 pp_common::errcode 风格的错误码(前端按码本地化)。
    pub fn code(&self) -> &'static str {
        match self {
            ProviderError::MissingKey => "provider_missing_key",
            ProviderError::Auth => "provider_auth",
            ProviderError::InsufficientBalance => "provider_balance",
            ProviderError::RateLimited => "provider_rate_limited",
            ProviderError::Http { .. } => "provider_http",
            ProviderError::Network(_) => "provider_network",
            ProviderError::BadResponse(_) => "provider_bad_response",
        }
    }

    /// 值不值得自动重试。密钥、余额类问题重试没有意义,只会白白耗时。
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::RateLimited | ProviderError::Network(_) => true,
            ProviderError::Http { status, .. } => *status >= 500,
            // 模型偶尔会吐空内容或坏 JSON,再问一次通常就好了
            ProviderError::BadResponse(_) => true,
            ProviderError::MissingKey | ProviderError::Auth | ProviderError::InsufficientBalance => false,
        }
    }

    /// HTTP 状态码 → 错误。`body` 只保留开头一段,避免把整页 HTML 塞进日志。
    pub fn from_status(status: u16, body: &str) -> Self {
        match status {
            401 | 403 => ProviderError::Auth,
            402 => ProviderError::InsufficientBalance,
            429 => ProviderError::RateLimited,
            _ => ProviderError::Http {
                status,
                body: body.chars().take(300).collect(),
            },
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::MissingKey => write!(f, "no API key configured"),
            ProviderError::Auth => write!(f, "API key rejected"),
            ProviderError::InsufficientBalance => write!(f, "insufficient balance"),
            ProviderError::RateLimited => write!(f, "rate limited"),
            ProviderError::Http { status, body } => write!(f, "HTTP {status}: {body}"),
            ProviderError::Network(e) => write!(f, "network error: {e}"),
            ProviderError::BadResponse(e) => write!(f, "unusable response: {e}"),
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_map_to_actionable_errors() {
        assert_eq!(ProviderError::from_status(401, ""), ProviderError::Auth);
        assert_eq!(ProviderError::from_status(402, ""), ProviderError::InsufficientBalance);
        assert_eq!(ProviderError::from_status(429, ""), ProviderError::RateLimited);
        assert!(matches!(ProviderError::from_status(503, "busy"), ProviderError::Http { status: 503, .. }));
    }

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(ProviderError::RateLimited.retryable());
        assert!(ProviderError::Network("timeout".into()).retryable());
        assert!(ProviderError::from_status(502, "").retryable());
        assert!(!ProviderError::from_status(400, "").retryable());
        assert!(!ProviderError::Auth.retryable());
        assert!(!ProviderError::InsufficientBalance.retryable());
        assert!(!ProviderError::MissingKey.retryable());
    }

    #[test]
    fn error_bodies_are_truncated() {
        let long = "x".repeat(5000);
        let ProviderError::Http { body, .. } = ProviderError::from_status(500, &long) else {
            panic!("expected Http");
        };
        assert_eq!(body.len(), 300);
    }
}
