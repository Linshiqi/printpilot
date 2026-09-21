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

/// 下大文件用的客户端:没有总时长上限(几百 MB 在慢网上要很久),靠读超时发现死连接。
pub fn build_download_client(proxy: Option<&str>) -> Result<reqwest::Client, ProviderError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("PrintPilot/", env!("CARGO_PKG_VERSION")));
    if let Some(url) = proxy.map(str::trim).filter(|s| !s.is_empty()) {
        let proxy = reqwest::Proxy::all(url).map_err(|e| ProviderError::Network(format!("bad proxy: {e}")))?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| ProviderError::Network(e.to_string()))
}

/// 把一个文件下到 `dest`。先写 `<dest>.part`,下完再换名——半截的文件不会被当成下好了的。
/// `on_progress(已下字节, 总字节)`;总数未知时是 0。只认 https(本机调试用的 localhost 例外)。
/// 下到的内容对不对由调用方校验(SHA-256):这里只管搬运。
pub async fn download_to(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    on_progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<u64, ProviderError> {
    use std::io::Write;
    check_base_url(url)?;
    let mut resp = client.get(url).send().await.map_err(|e| ProviderError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(ProviderError::Http { status, body: format!("GET {url}") });
    }
    let total = resp.content_length().unwrap_or(0);
    let part = dest.with_extension("part");
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ProviderError::Network(e.to_string()))?;
    }
    let result = async {
        let mut file = std::fs::File::create(&part).map_err(|e| ProviderError::Network(e.to_string()))?;
        let mut done = 0u64;
        while let Some(chunk) = resp.chunk().await.map_err(|e| ProviderError::Network(e.to_string()))? {
            file.write_all(&chunk).map_err(|e| ProviderError::Network(e.to_string()))?;
            done += chunk.len() as u64;
            on_progress(done, total);
        }
        file.flush().map_err(|e| ProviderError::Network(e.to_string()))?;
        if total > 0 && done != total {
            return Err(ProviderError::Network(format!("connection closed early: {done} of {total} bytes")));
        }
        Ok(done)
    }
    .await;
    match result {
        Ok(done) => {
            std::fs::rename(&part, dest).map_err(|e| ProviderError::Network(e.to_string()))?;
            Ok(done)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
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

    /// 一个只回固定内容的本机 HTTP 服务(一次连接一个响应)。
    fn serve_once(status: &'static str, body: Vec<u8>, declared_len: usize) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(&body);
            }
        });
        port
    }

    #[tokio::test]
    async fn downloads_land_atomically_and_report_progress() {
        let dir = std::env::temp_dir().join(format!("pp-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("pack.tar.zst");
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let port = serve_once("200 OK", body.clone(), body.len());
        let seen = std::sync::Mutex::new((0u64, 0u64));
        let client = build_download_client(None).unwrap();
        let n = download_to(&client, &format!("http://127.0.0.1:{port}/pack"), &dest, &|done, total| *seen.lock().unwrap() = (done, total)).await.unwrap();
        assert_eq!(n, body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(*seen.lock().unwrap(), (body.len() as u64, body.len() as u64));
        assert!(!dest.with_extension("part").exists());

        // 404、半截断开:都不留下文件
        let port = serve_once("404 Not Found", b"nope".to_vec(), 4);
        let missing = dir.join("missing.bin");
        assert!(matches!(download_to(&client, &format!("http://127.0.0.1:{port}/x"), &missing, &|_, _| {}).await, Err(ProviderError::Http { status: 404, .. })));
        let port = serve_once("200 OK", vec![1, 2, 3], 1000);
        let cut = dir.join("cut.bin");
        assert!(download_to(&client, &format!("http://127.0.0.1:{port}/x"), &cut, &|_, _| {}).await.is_err(), "声明 1000 字节只给了 3 个");
        assert!(!missing.exists() && !cut.exists() && !cut.with_extension("part").exists());

        // 明文的外部地址不下
        assert!(download_to(&client, "http://example.com/pack", &dir.join("x"), &|_, _| {}).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
