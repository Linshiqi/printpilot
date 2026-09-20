//! 所有适配器共用的 HTTP 客户端:统一的超时、代理与 User-Agent。

use std::time::Duration;

use crate::error::ProviderError;

/// `proxy`:用户在设置里填的代理(`http://…` / `socks5://…`);`None` = 跟随系统代理。
pub fn build_client(proxy: Option<&str>) -> Result<reqwest::Client, ProviderError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // 带思考的大模型写一段建模代码,非流式可能要好几分钟:总时长放到 10 分钟
        //(DeepSeek 自己的上限也是「10 分钟还没开始推理就断开」)。
        // 真正防「连接死了还在傻等」的是读超时:DeepSeek 在非流式请求等待期间会持续发空行保活
        //(官方 rate_limit 文档,2026-09-20 核对),所以 120 秒收不到任何字节 = 连接已经坏了。
        .timeout(Duration::from_secs(600))
        .read_timeout(Duration::from_secs(120))
        .user_agent(concat!("PrintPilot/", env!("CARGO_PKG_VERSION")));
    if let Some(url) = proxy.map(str::trim).filter(|s| !s.is_empty()) {
        let proxy = reqwest::Proxy::all(url).map_err(|e| ProviderError::Network(format!("bad proxy: {e}")))?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| ProviderError::Network(e.to_string()))
}

/// 接口地址必须是 https(本机调试用的 localhost 例外)——密钥不能在明文链路上跑。velo 同款规则。
pub fn check_base_url(url: &str) -> Result<(), ProviderError> {
    let lower = url.trim().to_ascii_lowercase();
    // 取出主机名再比较:只看前缀的话 `http://localhost.evil.com` 会混过去
    let local = lower.strip_prefix("http://").is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = match authority.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or(""),
            None => authority.split(':').next().unwrap_or(""),
        };
        matches!(host, "localhost" | "127.0.0.1" | "::1")
    });
    if lower.starts_with("https://") || local {
        Ok(())
    } else {
        Err(ProviderError::BadResponse(format!("base url must be https: {url}")))
    }
}

/// 发一个 JSON POST,拿回 JSON。非 2xx 转成 `ProviderError`。
pub async fn post_json(
    client: &reqwest::Client,
    url: &str,
    bearer: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, ProviderError> {
    let resp = client
        .post(url)
        .bearer_auth(bearer)
        .json(body)
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| ProviderError::Network(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(ProviderError::from_status(status, &text));
    }
    serde_json::from_str(&text).map_err(|e| ProviderError::BadResponse(format!("not JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_localhost_is_accepted() {
        assert!(check_base_url("https://api.deepseek.com").is_ok());
        assert!(check_base_url("http://localhost:11434/v1").is_ok());
        assert!(check_base_url("http://127.0.0.1:8080").is_ok());
        assert!(check_base_url("http://api.example.com").is_err());
        assert!(check_base_url("ftp://x").is_err());
        assert!(check_base_url("http://[::1]:8080/v1").is_ok());
        assert!(check_base_url("HTTP://EVIL.example.com").is_err());
        // 以 localhost 开头的域名不是本机
        assert!(check_base_url("http://localhost.evil.com/v1").is_err());
        assert!(check_base_url("http://127.0.0.1.evil.com").is_err());
    }

    #[test]
    fn client_builds_with_and_without_a_proxy() {
        assert!(build_client(None).is_ok());
        assert!(build_client(Some("  ")).is_ok(), "空白代理 = 没填");
        assert!(build_client(Some("socks5://127.0.0.1:7890")).is_ok());
        assert!(build_client(Some("not a url")).is_err());
    }
}
