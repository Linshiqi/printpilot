//! App 根组件:建状态、恢复偏好、首次加载、整体布局。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::t_string;
use pp_common::AppInfo;

use crate::controller::ProjectController;
use crate::i18n::{use_i18n, Locale};
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::{AppState, DroppedFiles, Route};
use crate::theme::{apply_theme, get_pref, set_pref};
use crate::ui::{ContextMenuHost, Toasts};
use crate::view::board::BoardView;
use crate::view::imagery::ImageryView;
use crate::view::lab::LabView;
use crate::view::studio::StudioView;
use crate::view::placeholder::ComingSoon;
use crate::view::pricing::PricingView;
use crate::view::publish::PublishView;
use crate::view::project_drawer::ProjectDrawer;
use crate::view::research::ResearchView;
use crate::view::settings::SettingsView;
use crate::view::sidebar::Sidebar;

/// 新增语言:Cargo.toml 的 locales 列表 + locales/<code>.json + 这张表 + 设置页的语言选项。
const LOCALES: &[(Locale, &str)] = &[(Locale::zh, "zh"), (Locale::en, "en")];

fn locale_code(l: Locale) -> &'static str {
    LOCALES.iter().find(|(loc, _)| *loc == l).map(|(_, c)| *c).unwrap_or("zh")
}

/// 后端事件的监听统一在 App 里注册一次(velo 的做法):页面组件来来去去,监听器不跟着反复挂卸。
/// 每个 listener 就地定义自己的载荷结构。
fn listen_cad_engine_progress(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Payload {
        phase: String,
        #[serde(default)]
        done: u64,
        #[serde(default)]
        total: u64,
    }

    let closure = Closure::wrap(Box::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        let Ok(p) = serde_wasm_bindgen::from_value::<Payload>(payload) else {
            return;
        };
        state.cad_engine_progress.set(Some((p.phase, p.done, p.total)));
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::CAD_ENGINE_PROGRESS, closure.into_js_value()).await;
    });
}

fn listen_cad_progress(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Payload {
        step: String,
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        problems: u32,
    }

    let closure = Closure::wrap(Box::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        let Ok(p) = serde_wasm_bindgen::from_value::<Payload>(payload) else {
            return;
        };
        // 「开始写第 N 版」= 一次新的模型调用:上一次回答的片段到此为止
        if p.step == "writing_code" {
            state.cad_stream.set(Default::default());
        }
        state.cad_progress.set(Some((p.step, p.attempt, p.problems)));
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::CAD_PROGRESS, closure.into_js_value()).await;
    });
}

fn listen_cad_stream(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Payload {
        kind: String,
        #[serde(default)]
        text: String,
    }

    let closure = Closure::wrap(Box::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        let Ok(p) = serde_wasm_bindgen::from_value::<Payload>(payload) else {
            return;
        };
        state.cad_stream.update(|live| match p.kind.as_str() {
            "reasoning" => live.reasoning.push_str(&p.text),
            "content" => live.content.push_str(&p.text),
            // 刚才那次回答作废了(坏响应,要重问)
            _ => *live = Default::default(),
        });
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::CAD_STREAM, closure.into_js_value()).await;
    });
}

fn listen_image_progress(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Payload {
        phase: String,
        #[serde(default)]
        provider: String,
        #[serde(default)]
        count: u32,
    }

    let closure = Closure::wrap(Box::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        let Ok(p) = serde_wasm_bindgen::from_value::<Payload>(payload) else {
            return;
        };
        state.image_progress.set(Some((p.phase, p.provider, p.count)));
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::IMAGE_PROGRESS, closure.into_js_value()).await;
    });
}

fn listen_update_progress(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Payload {
        #[serde(default)]
        downloaded: u64,
        #[serde(default)]
        total: u64,
    }

    let closure = Closure::wrap(Box::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        if let Ok(p) = serde_wasm_bindgen::from_value::<Payload>(payload) {
            state.update_progress.set(Some((p.downloaded, p.total)));
        }
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::UPDATE_PROGRESS, closure.into_js_value()).await;
    });
}

/// 把文件拖进窗口。WebView 自己的 HTML5 拖放收不到本地文件的路径,用的是 Tauri 的原生拖放事件:
/// 悬着的时候更新 `state.file_drag`(页面据此画「松开导入」的提示),松手时交给当前页面登记的处理函数。
/// 坐标是物理像素,换成 CSS 像素再给页面——页面要拿它和元素的位置比。
fn listen_file_drops(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Position {
        x: f64,
        y: f64,
    }
    #[derive(serde::Deserialize)]
    struct Payload {
        #[serde(default)]
        paths: Option<Vec<String>>,
        position: Position,
    }

    fn read(event: &JsValue) -> Option<(Vec<String>, f64, f64)> {
        let payload = js_sys::Reflect::get(event, &JsValue::from_str("payload")).ok()?;
        let p: Payload = serde_wasm_bindgen::from_value(payload).ok()?;
        let scale = web_sys::window().map(|w| w.device_pixel_ratio()).filter(|s| *s > 0.0).unwrap_or(1.0);
        Some((p.paths.unwrap_or_default(), p.position.x / scale, p.position.y / scale))
    }

    let listen = move |name: &'static str, handle: Box<dyn FnMut(JsValue)>| {
        let closure = Closure::wrap(handle);
        spawn_local(async move {
            let _ = ipc::listen(name, closure.into_js_value()).await;
        });
    };
    listen(
        ipc::event::DRAG_ENTER,
        Box::new(move |event| {
            if let Some((paths, x, y)) = read(&event) {
                state.file_drag.set(Some(DroppedFiles { paths, x, y }));
            }
        }),
    );
    listen(
        ipc::event::DRAG_OVER,
        Box::new(move |event| {
            if let Some((_, x, y)) = read(&event) {
                state.file_drag.update(|drag| {
                    if let Some(drag) = drag {
                        (drag.x, drag.y) = (x, y);
                    }
                });
            }
        }),
    );
    listen(ipc::event::DRAG_LEAVE, Box::new(move |_| state.file_drag.set(None)));
    listen(
        ipc::event::DRAG_DROP,
        Box::new(move |event| {
            state.file_drag.set(None);
            let Some((paths, x, y)) = read(&event) else { return };
            if paths.is_empty() {
                return;
            }
            match state.file_drop_handler.get_value() {
                Some((_, handler)) => handler.run(DroppedFiles { paths, x, y }),
                // 这个页面不收文件:拖进来的是图片的话,告诉用户该拖到哪
                None if paths.iter().any(|p| pp_common::imagery::is_importable_image(p)) => {
                    state.notify_info(leptos_i18n::td_string!(crate::i18n_util::current_locale(), imagery.drop_elsewhere));
                }
                None => {}
            }
        }),
    );
}

fn listen_research_progress(state: AppState) {
    use wasm_bindgen::prelude::*;

    #[derive(serde::Deserialize)]
    struct Payload {
        step: String,
        #[serde(default)]
        done: u32,
        #[serde(default)]
        total: u32,
    }

    let closure = Closure::wrap(Box::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        let Ok(p) = serde_wasm_bindgen::from_value::<Payload>(payload) else {
            return;
        };
        state.research_progress.set(Some((p.step, p.done, p.total)));
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::RESEARCH_PROGRESS, closure.into_js_value()).await;
    });
}

#[component]
pub fn App() -> impl IntoView {
    let i18n = use_i18n();
    let state = AppState::new();
    provide_context(state);

    // 语言:用户选过的优先,否则默认中文
    if let Some(saved) = get_pref("locale").and_then(|code| LOCALES.iter().find(|(_, c)| *c == code)) {
        i18n.set_locale(saved.0);
    }
    Effect::new(move |_| {
        let cur = i18n.get_locale();
        set_pref("locale", locale_code(cur));
        crate::i18n_util::set_current_locale(cur);
        if let Some(root) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
            let _ = root.set_attribute("lang", locale_code(cur));
        }
    });

    Effect::new(move |_| apply_theme(state.theme.get()));

    // 「已停留 N 天」靠这个时钟刷新;一分钟一次足够
    set_interval(
        move || state.now_ms.set(crate::utils::now_ms()),
        std::time::Duration::from_secs(60),
    );

    ProjectController::new(state).load();
    state.reload_providers();
    listen_research_progress(state);
    listen_cad_progress(state);
    listen_cad_stream(state);
    listen_cad_engine_progress(state);
    listen_image_progress(state);
    listen_update_progress(state);
    listen_file_drops(state);
    // 启动几秒后静默查一次更新:查不到(断网、地址还没配好)什么都不说;有新版本才提示一句
    set_timeout(
        move || {
            spawn_local(async move {
                #[derive(serde::Deserialize)]
                struct Found {
                    version: String,
                    #[serde(default)]
                    notes: String,
                }
                if let Ok(Some(found)) = ipc::call_no_args::<Option<Found>>(cmd::UPDATE_CHECK).await {
                    state.notify_info(leptos_i18n::td_string!(crate::i18n_util::current_locale(), settings.update_found_toast, version = found.version.clone()).to_string());
                    state.update_available.set(Some((found.version, found.notes)));
                }
            });
        },
        std::time::Duration::from_secs(6),
    );
    // 浏览器自带的右键菜单一律拦掉,换成应用自己的(ui::context_menu)
    crate::ui::context_menu::install(state);
    spawn_local(async move {
        match ipc::call_no_args::<AppInfo>(cmd::APP_INFO).await {
            Ok(info) => state.app_info.set(Some(info)),
            Err(e) => state.notify_error(e),
        }
    });

    view! {
        <div class="h-screen w-screen flex bg-gray-50 dark:bg-gray-900 text-gray-900 dark:text-gray-100">
            <Sidebar state=state/>
            <main class="flex-1 min-w-0 h-full">
                {move || match state.route.get() {
                    Route::Projects => view! { <BoardView state=state/> }.into_any(),
                    Route::Lab => view! { <LabView state=state/> }.into_any(),
                    Route::Images => view! { <ImageryView state=state/> }.into_any(),
                    Route::Studio => view! { <StudioView state=state/> }.into_any(),
                    Route::Pricing => view! { <PricingView state=state/> }.into_any(),
                    Route::Publish => view! { <PublishView state=state/> }.into_any(),
                    Route::Settings => view! { <SettingsView state=state/> }.into_any(),
                    Route::Dashboard => view! {
                        <ComingSoon icon=IconKind::Dashboard title=move || t_string!(i18n, nav.dashboard)/>
                    }.into_any(),
                    Route::Ideas => view! { <ResearchView state=state/> }.into_any(),
                    Route::Calendar => view! {
                        <ComingSoon icon=IconKind::Calendar title=move || t_string!(i18n, nav.calendar)/>
                    }.into_any(),
                    Route::Orders => view! {
                        <ComingSoon icon=IconKind::Package title=move || t_string!(i18n, nav.orders)/>
                    }.into_any(),
                    Route::Analytics => view! {
                        <ComingSoon icon=IconKind::Chart title=move || t_string!(i18n, nav.analytics)/>
                    }.into_any(),
                }}
            </main>
            <ProjectDrawer state=state/>
            <Toasts state=state/>
            <ContextMenuHost state=state/>
        </div>
    }
}
