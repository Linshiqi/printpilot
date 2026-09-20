mod app;
mod controller;
#[cfg(test)]
mod i18n_guard;
mod i18n_util;
mod icon;
mod ipc;
mod pointer_drag;
mod state;
mod theme;
mod ui;
mod utils;
mod view;
mod viewer3d;

leptos_i18n::load_locales!();

use leptos::prelude::*;

fn current_window_label() -> String {
    use wasm_bindgen::JsCast;
    let get = |o: &wasm_bindgen::JsValue, k: &str| js_sys::Reflect::get(o, &k.into()).ok();
    (|| {
        let w = web_sys::window()?;
        let tauri = get(w.as_ref(), "__TAURI__")?;
        let win_mod = get(&tauri, "window")?;
        let f: js_sys::Function = get(&win_mod, "getCurrentWindow")?.dyn_into().ok()?;
        let cur = f.call0(&win_mod).ok()?;
        get(&cur, "label")?.as_string()
    })()
    .unwrap_or_else(|| "main".into())
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

fn bundle_name() -> String {
    let Some(perf) = web_sys::window().and_then(|w| w.performance()) else {
        return String::new();
    };
    perf.get_entries_by_type("resource")
        .iter()
        .filter_map(|e| js_sys::Reflect::get(&e, &wasm_bindgen::JsValue::from_str("name")).ok())
        .filter_map(|n| n.as_string())
        .find(|n| n.ends_with("_bg.wasm"))
        .and_then(|n| n.rsplit('/').next().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn report_boot(label: &str, wasm_ms: f64, mount_ms: f64) {
    let args = serde_json::json!({
        "label": label,
        "wasm_ms": wasm_ms,
        "mount_ms": mount_ms,
        "bundle": bundle_name(),
    });
    leptos::task::spawn_local(async move {
        let _ = ipc::call_unit(ipc::cmd::LOG_BOOT, &args).await;
    });
}

/// 前端日志 → 后端日志文件。前端没有自己的日志文件,出了问题只有这一条路能留下证据。
pub fn main_log(label: &str, message: &str) {
    ipc::fire(
        ipc::cmd::LOG_CLIENT_ERROR,
        &serde_json::json!({ "label": label, "message": message }),
    );
}

/// 未捕获异常与未处理的 Promise rejection 上报(带配额,防止一个死循环刷爆日志)。
fn install_error_reporter(label: &str) {
    use wasm_bindgen::JsCast;
    static LEFT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(20);
    let Some(win) = web_sys::window() else { return };

    let report = |label: String, msg: String| {
        if LEFT
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |v| v.checked_sub(1),
            )
            .is_err()
        {
            return;
        }
        main_log(&label, &msg);
    };

    let l1 = label.to_string();
    let on_err = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::wrap(Box::new(
        move |ev: web_sys::Event| {
            let msg = match ev.dyn_ref::<web_sys::ErrorEvent>() {
                Some(e) => format!("{} @{}:{}", e.message(), e.filename(), e.lineno()),
                None => "unknown error event".into(),
            };
            report(l1.clone(), msg);
        },
    ));
    let _ = win.add_event_listener_with_callback("error", on_err.as_ref().unchecked_ref());
    on_err.forget();

    let l2 = label.to_string();
    let on_rej = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::PromiseRejectionEvent)>::wrap(
        Box::new(move |ev: web_sys::PromiseRejectionEvent| {
            let reason = ev.reason();
            let msg = reason
                .as_string()
                .unwrap_or_else(|| js_sys::JsString::from(reason).as_string().unwrap_or_default());
            report(l2.clone(), format!("unhandledrejection: {msg}"));
        }),
    );
    let _ = win.add_event_listener_with_callback("unhandledrejection", on_rej.as_ref().unchecked_ref());
    on_rej.forget();
}

/// panic 上报:先走 console_error_panic_hook,再抓一份 JS 调用栈发给后端。
/// ⚠️ wasm **栈溢出不是 panic 是 trap**,到不了这里——那类问题用 scripts/cdp.py 连 WebView 抓。
fn install_panic_reporter(label: &str) {
    static LEFT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(5);
    let label = label.to_string();
    std::panic::set_hook(Box::new(move |info| {
        console_error_panic_hook::hook(info);
        if LEFT
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |v| v.checked_sub(1),
            )
            .is_err()
        {
            return;
        }
        let trace = js_sys::Error::new("trace");
        let stack: String = js_sys::Reflect::get(&trace, &wasm_bindgen::JsValue::from_str("stack"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .chars()
            .take(1500)
            .collect();
        main_log(&label, &format!("panic: {info}\n{stack}"));
    }));
}

fn drop_boot_splash() {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("boot-splash"))
    {
        el.remove();
    }
}

fn main() {
    let t_wasm = now_ms();
    let label = current_window_label();
    install_error_reporter(&label);
    install_panic_reporter(&label);
    // 以后的独立窗口(3D 工作室:index.html?win=studio&asset=<id>)在这里按 query 分派根组件,做法见 velo 的 main.rs
    mount_to_body(|| {
        view! {
            <crate::i18n::I18nContextProvider>
                <app::App/>
            </crate::i18n::I18nContextProvider>
        }
    });
    drop_boot_splash();
    report_boot(&label, t_wasm, now_ms() - t_wasm);
}
