//! 发布包的「手机页」:一个临时的、只读的局域网页面(docs/adr/0009-publish-pack.md)。
//!
//! 手机扫码打开,页面上只有几个大按钮(复制标题 / 正文 / 话题)和处理好的图片(长按保存)。
//! 「发布」这一下仍然由人在小红书 App 里点——这里不碰任何平台。
//!
//! - 按需开启、用完即停:关掉发布包面板、换一个包、空闲超过 20 分钟、应用退出,都会停;
//! - 监听端口由系统随机分配(CLAUDE.md:本地监听端口一律随机或先探测);
//! - 路径里带一个 128 位的一次性令牌;令牌不对一律 404,看不出这里有什么;
//! - 只读:只认 GET,只发这一个包里的文字和图片;
//! - 只用标准库:一个连接一个请求,读写都有超时。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const IDLE_LIMIT: Duration = Duration::from_secs(20 * 60);
/// 等请求行最多等这么久。浏览器会预先多开几条连接、什么都不发(手机上也一样):
/// 连接是一条一条串行处理的,每条空闲连接都等 5 秒的话,「停止」和下一张图都会被拖住十几秒
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1200);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// 停止时最多等服务线程这么久;还没退出就不等了(它自己会在下一轮循环里看到停止标记)
const STOP_WAIT: Duration = Duration::from_millis(2500);
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// 页面上要给出去的东西。
#[derive(Debug, Clone, Default)]
pub struct SharePayload {
    pub title: String,
    pub body: String,
    /// 已经拼好的那一行:`#桌搭 #3D打印`
    pub tags_line: String,
    /// 图片文件(按顺序)
    pub images: Vec<PathBuf>,
    /// 页面语言:`zh` / `en`
    pub lang: String,
}

pub struct ShareHandle {
    pub url: String,
    pub port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ShareHandle {
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for ShareHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(t) = self.thread.take() else { return };
        let deadline = Instant::now() + STOP_WAIT;
        while !t.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if t.is_finished() {
            let _ = t.join();
        }
    }
}

/// 128 位随机令牌(十六进制)。
fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    // 标准库没有现成的随机数;RandomState 的种子来自操作系统的随机源,每个实例都不一样
    let mut out = String::with_capacity(32);
    for salt in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt ^ std::process::id() as u64);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// 本机在局域网里的地址:向一个公网地址「连接」一个 UDP 套接字(不发任何包),看系统选了哪块网卡。
pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(223, 5, 5, 5), 53)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// 开始分享。`lan = false` 只监听本机回环(测试用:不会触发系统防火墙的授权提示)。
pub fn start(payload: SharePayload, lan: bool) -> std::io::Result<ShareHandle> {
    let bind_ip = if lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let host = if lan { lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let token = new_token();
    let url = format!("http://{host}:{port}/{token}/");

    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::Builder::new().name("pp-share".into()).spawn(move || {
        let mut last_used = Instant::now();
        while !flag.load(Ordering::SeqCst) && last_used.elapsed() < IDLE_LIMIT {
            match listener.accept() {
                Ok((stream, _)) => {
                    if serve(stream, &token, &payload) {
                        last_used = Instant::now();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(80)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        log::info!("[share] 手机页已停止");
    })?;
    log::info!("[share] 手机页已开启 · 端口 {port} · {}", if lan { "局域网" } else { "仅本机" });
    Ok(ShareHandle {
        url,
        port,
        stop,
        thread: Some(thread),
    })
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// 处理一个连接。返回这是不是一次带着正确令牌的请求(用来刷新空闲计时)。
fn serve(mut stream: TcpStream, token: &str, payload: &SharePayload) -> bool {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&stream).take(MAX_REQUEST_BYTES as u64);
        if reader.read_line(&mut line).is_err() {
            return false;
        }
        // 把请求头读完(不关心内容),免得对方收到 RST
        let mut header = String::new();
        while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
            header.clear();
        }
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let Some(rest) = path.strip_prefix('/').and_then(|p| p.strip_prefix(token)) else {
        respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not found");
        return false;
    };
    if method != "GET" {
        respond(&mut stream, "405 Method Not Allowed", "text/plain; charset=utf-8", b"read only");
        return true;
    }
    match rest {
        "" | "/" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", page(payload).as_bytes()),
        other => {
            let image = other.strip_prefix("/img/").and_then(|n| n.parse::<usize>().ok()).and_then(|n| payload.images.get(n));
            match image.and_then(|p| std::fs::read(p).ok()) {
                Some(bytes) => respond(&mut stream, "200 OK", "image/jpeg", &bytes),
                None => respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not found"),
            }
        }
    }
    true
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 文字放进 `<script type="application/json">`:JSON 本身是安全的,只要不让 `</script>` 提前结束它。
fn json_for_script(value: &serde_json::Value) -> String {
    value.to_string().replace("</", "<\\/")
}

fn page(p: &SharePayload) -> String {
    let zh = p.lang != "en";
    let t = |zh_text: &'static str, en_text: &'static str| if zh { zh_text } else { en_text };
    let mut buttons = String::new();
    for (key, label, text) in [
        ("title", t("复制标题", "Copy title"), &p.title),
        ("body", t("复制正文", "Copy text"), &p.body),
        ("tags", t("复制话题", "Copy topics"), &p.tags_line),
    ] {
        if !text.trim().is_empty() {
            buttons.push_str(&format!("<button class=\"copy\" data-key=\"{key}\">{label}</button>"));
        }
    }
    let images: String = (0..p.images.len()).map(|i| format!("<img src=\"img/{i}\" alt=\"{}\" loading=\"lazy\">", i + 1)).collect();
    let data = json_for_script(&serde_json::json!({ "title": p.title, "body": p.body, "tags": p.tags_line, "copied": t("已复制 ✓", "Copied ✓"), "failed": t("复制失败,请长按下面的文字手动复制", "Copy failed — long-press the text below") }));
    format!(
        r##"<!doctype html><html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>{title}</title><style>
*{{box-sizing:border-box}}body{{margin:0;padding:16px 16px 40px;font:16px/1.6 -apple-system,BLinkMacSystemFont,"PingFang SC","Microsoft YaHei",sans-serif;background:#f6f7f9;color:#111827}}
h1{{font-size:17px;margin:0 0 4px}}p.hint{{margin:0 0 14px;font-size:13px;color:#6b7280}}
button.copy{{display:block;width:100%;height:56px;margin:0 0 10px;border:0;border-radius:14px;background:#4f46e5;color:#fff;font-size:18px;font-weight:600}}
button.copy.done{{background:#16a34a}}button.copy.next{{box-shadow:0 0 0 3px #c7d2fe}}
h2{{font-size:14px;margin:22px 0 8px;color:#374151}}img{{display:block;width:100%;height:auto;margin:0 0 10px;border-radius:12px;background:#e5e7eb;-webkit-touch-callout:default}}
pre{{white-space:pre-wrap;word-break:break-word;margin:0 0 10px;padding:12px;border-radius:12px;background:#fff;font:inherit;font-size:14px;-webkit-user-select:text;user-select:text}}
</style></head><body>
<h1>{title}</h1><p class="hint">{hint}</p>
{buttons}
<h2>{images_title}</h2>{images}
<h2>{text_title}</h2><pre id="t-title"></pre><pre id="t-body"></pre><pre id="t-tags"></pre>
<script type="application/json" id="data">{data}</script>
<script>
var D=JSON.parse(document.getElementById('data').textContent);
['title','body','tags'].forEach(function(k){{var el=document.getElementById('t-'+k);if(D[k]){{el.textContent=D[k]}}else{{el.style.display='none'}}}});
function fallback(text){{var a=document.createElement('textarea');a.value=text;a.setAttribute('readonly','');a.style.position='fixed';a.style.opacity='0';document.body.appendChild(a);a.select();a.setSelectionRange(0,text.length);var ok=false;try{{ok=document.execCommand('copy')}}catch(e){{}}document.body.removeChild(a);return ok}}
function copy(text){{if(navigator.clipboard&&window.isSecureContext){{return navigator.clipboard.writeText(text).then(function(){{return true}},function(){{return fallback(text)}})}}return Promise.resolve(fallback(text))}}
var buttons=[].slice.call(document.querySelectorAll('button.copy'));
if(buttons[0])buttons[0].classList.add('next');
buttons.forEach(function(b,i){{var label=b.textContent;b.addEventListener('click',function(){{copy(D[b.dataset.key]).then(function(ok){{b.textContent=ok?D.copied:D.failed;if(ok){{b.classList.add('done');buttons.forEach(function(x){{x.classList.remove('next')}});if(buttons[i+1])buttons[i+1].classList.add('next')}}setTimeout(function(){{b.textContent=label}},1800)}})}})}});
</script></body></html>"##,
        lang = if zh { "zh" } else { "en" },
        title = html_escape(if p.title.trim().is_empty() { t("发布包", "Publish pack") } else { p.title.trim() }),
        hint = t("按顺序复制,到小红书里粘贴;图片长按保存到相册。发布由你自己点。", "Copy in order and paste into the app; long-press an image to save it. You press Publish yourself."),
        buttons = buttons,
        images_title = t("图片(长按保存)", "Images (long-press to save)"),
        images = images,
        text_title = t("文字(复制按钮不灵时,长按这里手动复制)", "Text (long-press to copy manually if the buttons fail)"),
        data = data,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str, method: &str) -> (String, Vec<u8>) {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nUser-Agent: test\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&raw[..split]).into_owned(), raw[split + 4..].to_vec())
    }

    fn payload(dir: &std::path::Path) -> SharePayload {
        std::fs::create_dir_all(dir).unwrap();
        let img = dir.join("01.jpg");
        std::fs::write(&img, b"\xff\xd8fake-jpeg").unwrap();
        SharePayload {
            title: "桌面乱线终结者".into(),
            body: "第一段\n</script><script>alert(1)</script>".into(),
            tags_line: "#桌搭 #3D打印".into(),
            images: vec![img],
            lang: "zh".into(),
        }
    }

    #[test]
    fn the_page_is_only_reachable_with_the_token_and_is_read_only() {
        let dir = std::env::temp_dir().join(format!("pp-share-test-{}", std::process::id()));
        let handle = start(payload(&dir), false).unwrap();
        assert!(handle.url.starts_with("http://127.0.0.1:") && handle.is_running());
        let token_path = handle.url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap();
        assert_eq!(token_path.len(), 1 + 32 + 1, "128 位令牌:{token_path}");

        let (head, body) = get(handle.port, &token_path, "GET");
        let html = String::from_utf8(body).unwrap();
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("no-store") && head.contains("default-src 'none'"), "{head}");
        assert!(html.contains("复制标题") && html.contains("复制正文") && html.contains("复制话题") && html.contains("img/0"));
        assert!(!html.contains("</script><script>alert(1)"), "正文里的 </script> 不能把数据块提前结束");
        assert!(html.contains("<\\/script>"));

        let (head, body) = get(handle.port, &format!("{token_path}img/0"), "GET");
        assert!(head.contains("image/jpeg") && body.starts_with(b"\xff\xd8"));
        assert!(get(handle.port, &format!("{token_path}img/7"), "GET").0.starts_with("HTTP/1.1 404"));
        assert!(get(handle.port, &format!("{token_path}img/../../x"), "GET").0.starts_with("HTTP/1.1 404"));

        // 令牌不对:什么都看不到
        for path in ["/", "/00000000000000000000000000000000/", "/favicon.ico"] {
            assert!(get(handle.port, path, "GET").0.starts_with("HTTP/1.1 404"), "{path}");
        }
        assert!(get(handle.port, &token_path, "POST").0.starts_with("HTTP/1.1 405"));

        let port = handle.port;
        drop(handle);
        assert!(TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(500)).is_err(), "停了之后端口就关了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn idle_connections_do_not_hold_up_requests_or_stopping() {
        let dir = std::env::temp_dir().join(format!("pp-share-idle-{}", std::process::id()));
        let handle = start(payload(&dir), false).unwrap();
        let token_path = handle.url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap();
        // 浏览器预开的连接:连上了,什么都不发
        let idle: Vec<TcpStream> = (0..3).map(|_| TcpStream::connect((Ipv4Addr::LOCALHOST, handle.port)).unwrap()).collect();
        let started = Instant::now();
        assert!(get(handle.port, &token_path, "GET").0.starts_with("HTTP/1.1 200"));
        assert!(started.elapsed() < Duration::from_secs(5), "三条空闲连接排在前面,真正的请求等了 {:?}", started.elapsed());

        let _more: Vec<TcpStream> = (0..3).map(|_| TcpStream::connect((Ipv4Addr::LOCALHOST, handle.port)).unwrap()).collect();
        let started = Instant::now();
        drop(handle);
        assert!(started.elapsed() < Duration::from_secs(3), "停止不该被空闲连接拖住:{:?}", started.elapsed());
        drop(idle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_differ_and_empty_fields_get_no_button() {
        assert_ne!(new_token(), new_token());
        let html = page(&SharePayload { title: "只有标题".into(), lang: "en".into(), ..Default::default() });
        assert!(html.contains("Copy title") && !html.contains("Copy text") && !html.contains("Copy topics"));
        assert!(page(&SharePayload { title: "<b>x</b>".into(), ..Default::default() }).contains("&lt;b&gt;x&lt;/b&gt;"));
    }
}
