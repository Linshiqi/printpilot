# -*- coding: utf-8 -*-
"""连 WebView2 的远程调试口(CDP)抓前端证据 —— 纯标准库,不装任何包。

用法(先带调试口起应用,再跑脚本):
  .\scripts\dev.ps1                      # 开发版:脚本会挑一个空闲的调试口,并记在 target/dev-ports.json 里
  # 打包版要自己带口启动(⚠️ 同一 user-data-folder 已有不带口的浏览器进程在跑时参数不生效,先把它关干净):
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=17439 --remote-allow-origins=*'
  .\target\release\printpilot.exe

调试口怎么定:环境变量 CDP_PORT > target/dev-ports.json(dev.ps1 写的)> 17439。
刻意不用 9222 / 9333 这类常见值——本机同时开着几个 Tauri 工程时那一带最容易撞。

  python scripts/cdp.py targets                # 列页面
  python scripts/cdp.py console [秒]           # 回放最近控制台 + 之后持续抓 N 秒
  python scripts/cdp.py eval "<js 表达式>"     # 在页面里求值(返回 JSON)
  python scripts/cdp.py eval-to <文件> "<js>"  # 同上,但完整结果写进文件(eval 的输出会截到 4000 字)
  python scripts/cdp.py watch [秒]             # 刷新页面从零盯:第一条 panic(带 Rust 源位置)
  python scripts/cdp.py click-btn "<正则>"     # 按 aria-label/title 找按钮点一下,抓第一条异常全栈
  python scripts/cdp.py click <x> <y>          # 按坐标点
  python scripts/cdp.py drag <x1> <y1> <x2> <y2>  # 真实鼠标拖拽(看板卡片换列)
  python scripts/cdp.py type "<文本>"           # 往当前焦点输入文字
  python scripts/cdp.py shot <文件.png>         # 截图存盘

为什么要它:有一类前端故障**不进 printpilot.log**——wasm 栈溢出是陷阱不是 panic
(`RuntimeError: memory access out of bounds`),panic hook 根本不跑;DevTools
里那条异常带完整符号名(debug wasm 有 name 段),`click-btn` 会把栈去重后打出来,
直接指到出事的组件闭包(2026-09 "点设置整个 UI 死"就是这样定位的)。
Windows 控制台记得 `PYTHONIOENCODING=utf-8`,否则 GBK 编不出 emoji 会炸。
"""
import base64
import json
import os
import re
import socket
import struct
import sys
import time
import urllib.request
from collections import Counter

DEFAULT_PORT = 17439


def _resolve_port():
    env = os.environ.get("CDP_PORT")
    if env:
        return int(env)
    try:
        here = os.path.dirname(os.path.abspath(__file__))
        with open(os.path.join(here, "..", "target", "dev-ports.json"), encoding="utf-8") as f:
            port = int(json.load(f).get("cdp") or 0)
        if port:
            return port
    except (OSError, ValueError):
        pass
    return DEFAULT_PORT


PORT = _resolve_port()


def http_json(path):
    with urllib.request.urlopen(f"http://127.0.0.1:{PORT}{path}", timeout=5) as r:
        return json.loads(r.read().decode())


# ─── 最小 WebSocket 客户端(RFC 6455:握手 + 掩码帧 + 分片 + ping) ──────────
def ws_connect(url):
    assert url.startswith("ws://")
    hostport, _, path = url[5:].partition("/")
    host, _, port = hostport.partition(":")
    s = socket.create_connection((host, int(port or 80)), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall(
        (
            f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
            f"Origin: http://{hostport}\r\n\r\n"
        ).encode()
    )
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = s.recv(4096)
        if not chunk:
            raise EOFError("handshake EOF")
        buf += chunk
    if b"101" not in buf.split(b"\r\n", 1)[0]:
        raise RuntimeError("handshake failed: " + buf.decode(errors="replace")[:300])
    return s


def _masked(op, data: bytes):
    mask = os.urandom(4)
    n = len(data)
    h = bytes([0x80 | op])
    if n < 126:
        h += bytes([0x80 | n])
    elif n < 65536:
        h += bytes([0x80 | 126]) + struct.pack(">H", n)
    else:
        h += bytes([0x80 | 127]) + struct.pack(">Q", n)
    return h + mask + bytes(c ^ mask[i % 4] for i, c in enumerate(data))


def send_text(s, data: str):
    s.sendall(_masked(0x1, data.encode()))


def recv_exact(s, n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf


def recv_msg(s):
    payload = b""
    while True:
        h = recv_exact(s, 2)
        fin, op = h[0] & 0x80, h[0] & 0x0F
        n, masked = h[1] & 0x7F, h[1] & 0x80
        if n == 126:
            n = struct.unpack(">H", recv_exact(s, 2))[0]
        elif n == 127:
            n = struct.unpack(">Q", recv_exact(s, 8))[0]
        mkey = recv_exact(s, 4) if masked else None
        data = recv_exact(s, n)
        if mkey:
            data = bytes(c ^ mkey[i % 4] for i, c in enumerate(data))
        if op == 9:
            s.sendall(_masked(0xA, data))
            continue
        if op == 8:
            raise EOFError("closed")
        payload += data
        if fin:
            return payload.decode(errors="replace")


class Cdp:
    def __init__(self, ws_url):
        self.s = ws_connect(ws_url)
        self.next_id = 1
        self.events = []

    def call(self, method, params=None, timeout=15):
        mid = self.next_id
        self.next_id += 1
        send_text(self.s, json.dumps({"id": mid, "method": method, "params": params or {}}))
        end = time.time() + timeout
        self.s.settimeout(1.0)
        while time.time() < end:
            try:
                m = json.loads(recv_msg(self.s))
            except socket.timeout:
                continue
            if m.get("id") == mid:
                return m
            self.events.append(m)
        raise TimeoutError(method)

    def drain(self, secs):
        self.s.settimeout(0.5)
        end = time.time() + secs
        while time.time() < end:
            try:
                self.events.append(json.loads(recv_msg(self.s)))
            except socket.timeout:
                pass

    def eval(self, expr, timeout=20):
        # awaitPromise:表达式是 async IIFE 时拿到的是结果而不是 Promise 本身(调后端命令验证数据时要用)
        r = self.call("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True}, timeout)
        return r.get("result", {}).get("result", {}).get("value")


# ─── 输出 ───────────────────────────────────────────────────────────────────
def fmt_remote(v, limit=400):
    if "value" in v:
        return repr(v["value"])
    return v.get("description", v.get("type", "?"))[:limit]


def show_events(events, max_exc=3):
    exc = 0
    for m in events:
        meth = m.get("method", "")
        p = m.get("params", {})
        if meth == "Runtime.consoleAPICalled":
            args = " ".join(fmt_remote(a, 3000) for a in p.get("args", []))
            print(f"[console.{p.get('type')}] {args[:1500]}")
        elif meth == "Runtime.exceptionThrown":
            exc += 1
            if exc > max_exc:
                continue
            d = p.get("exceptionDetails", {})
            print(f"[exception] {d.get('text', '')} {fmt_remote(d.get('exception', {}))}")
            for f in d.get("stackTrace", {}).get("callFrames", [])[:8]:
                print(f"    at {f.get('functionName', '?')[:140]}")
        elif meth == "Log.entryAdded":
            e = p.get("entry", {})
            print(f"[log.{e.get('level')}] {e.get('text', '')[:600]}")
    if exc > max_exc:
        print(f"... 共 {exc} 条异常(只显示前 {max_exc})")


def show_first_exception(events):
    """第一条异常的完整栈:去重后逐帧打印,重复帧计数(递归/溢出特征),
    自有代码(printpilot_ui)的帧打 * 号。"""
    exs = [m for m in events if m.get("method") == "Runtime.exceptionThrown"]
    print(f"exceptions: {len(exs)}")
    if not exs:
        return
    desc = exs[0]["params"].get("exceptionDetails", {}).get("exception", {}).get("description", "")
    lines = desc.splitlines()
    print(lines[0] if lines else "?")
    print(f"# frames: {len(lines) - 1}")
    cnt = Counter(ln.split("(http")[0].strip()[:160] for ln in lines[1:])
    for k, n in cnt.most_common(6):
        if n > 1:
            print(f"  x{n}: {k}")
    print("# deepest first (dedup; * = printpilot_ui):")
    seen = set()
    for ln in lines[1:]:
        key = ln.split("(http")[0].strip()[:160]
        if key in seen:
            continue
        seen.add(key)
        mark = "*" if "printpilot_ui[" in ln and "::i18n::" not in ln else " "
        print(mark, key)


def page_target():
    # 调试口是这个应用专用的,口上的页面就是我们的页面;只需要跳过 DevTools 自己的窗口。
    # (不按开发服务器的端口号去匹配:那个端口可能被 dev.ps1 顺延过。)
    pages = [t for t in http_json("/json") if t.get("type") == "page" and not t.get("url", "").startswith("devtools://")]
    return pages[0] if pages else None


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "console"
    if cmd == "targets":
        for t in http_json("/json"):
            print(t.get("type"), "|", t.get("url"), "|", t.get("webSocketDebuggerUrl"))
        return
    t = page_target()
    if not t:
        print("no page target")
        return
    print("# target:", t.get("url"))
    c = Cdp(t["webSocketDebuggerUrl"])
    c.call("Runtime.enable")
    try:
        c.call("Log.enable")
    except Exception:
        pass

    if cmd == "console":
        c.drain(float(sys.argv[2]) if len(sys.argv) > 2 else 2.5)
        show_events(c.events)
    elif cmd == "eval":
        print(json.dumps(c.eval(sys.argv[2]), ensure_ascii=False, indent=1)[:4000])
        show_events(c.events)
    elif cmd == "eval-to":
        # 结果很大时(截图的 data URL 之类)写到文件里,不截断
        with open(sys.argv[2], "w", encoding="utf-8") as f:
            json.dump(c.eval(sys.argv[3]), f, ensure_ascii=False)
        print("saved", sys.argv[2])
    elif cmd == "watch":
        # 刷新页面从零盯:逮第一条 panic(带 Rust 源位置)与首个异常
        secs = float(sys.argv[2]) if len(sys.argv) > 2 else 90
        c.call("Page.enable")
        c.call("Page.reload", {"ignoreCache": False})
        c.events.clear()
        c.s.settimeout(0.5)
        end, t0, panics, exs = time.time() + secs, time.time(), 0, 0
        while time.time() < end:
            try:
                m = json.loads(recv_msg(c.s))
            except socket.timeout:
                continue
            meth, p = m.get("method", ""), m.get("params", {})
            ts = f"+{time.time() - t0:6.1f}s"
            if meth == "Runtime.consoleAPICalled":
                args = " ".join(fmt_remote(a, 3000) for a in p.get("args", []))
                if "panicked at" in args:
                    panics += 1
                    print(f"{ts} ### PANIC #{panics} ###\n{args[:6000]}")
                    if panics >= 2:
                        break
                elif p.get("type") == "error":
                    print(f"{ts} [console.error] {args[:800]}")
            elif meth == "Runtime.exceptionThrown":
                exs += 1
                if exs <= 3:
                    d = p.get("exceptionDetails", {})
                    print(f"{ts} [exception #{exs}] {fmt_remote(d.get('exception', {}), 300)}")
                    for f in d.get("stackTrace", {}).get("callFrames", [])[:15]:
                        print(f"    at {f.get('functionName', '?')[:150]}")
                if exs >= 6 and panics:
                    break
        print(f"# done: panics={panics} exceptions={exs}")
    elif cmd == "click-btn":
        pat = sys.argv[2]
        c.eval("Error.stackTraceLimit = 400")
        c.events.clear()
        js = (
            "(()=>{const re=new RegExp(%s,'i');"
            "const b=[...document.querySelectorAll('button')]"
            ".find(x=>re.test(x.getAttribute('aria-label')||x.title||x.textContent||''));"
            "if(!b)return 'no button';b.click();return 'clicked: '+(b.getAttribute('aria-label')||b.title||b.textContent).trim().slice(0,40)})()"
        ) % json.dumps(pat)
        print("click:", c.eval(js))
        c.drain(3)
        show_first_exception(c.events)
    elif cmd == "click":
        x, y = float(sys.argv[2]), float(sys.argv[3])
        for typ in ("mousePressed", "mouseReleased"):
            c.call("Input.dispatchMouseEvent", {"type": typ, "x": x, "y": y, "button": "left", "clickCount": 1})
        c.drain(1.5)
        show_events(c.events)
    elif cmd == "drag":
        # 真实鼠标事件(不是合成事件):pointer_drag.rs 要求 isPrimary、要过 6px 阈值、要能 setPointerCapture
        x1, y1, x2, y2 = (float(v) for v in sys.argv[2:6])
        c.call("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x1, "y": y1})
        c.call("Input.dispatchMouseEvent", {"type": "mousePressed", "x": x1, "y": y1, "button": "left", "buttons": 1, "clickCount": 1})
        steps = 12
        for i in range(1, steps + 1):
            x, y = x1 + (x2 - x1) * i / steps, y1 + (y2 - y1) * i / steps
            c.call("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x, "y": y, "button": "left", "buttons": 1})
            time.sleep(0.02)
        c.call("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": x2, "y": y2, "button": "left", "buttons": 0, "clickCount": 1})
        c.drain(1.5)
        show_events(c.events)
    elif cmd == "type":
        c.call("Input.insertText", {"text": sys.argv[2]})
        c.drain(0.5)
        show_events(c.events)
    elif cmd == "shot":
        c.call("Page.enable")
        r = c.call("Page.captureScreenshot", {"format": "png"}, timeout=30)
        data = r.get("result", {}).get("data")
        if not data:
            print("no screenshot:", json.dumps(r)[:300])
            return
        with open(sys.argv[2], "wb") as f:
            f.write(base64.b64decode(data))
        print("saved", sys.argv[2])
    else:
        print(__doc__)


if __name__ == "__main__":
    main()
