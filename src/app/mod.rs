//! App 根组件:建状态、恢复偏好、首次加载、整体布局。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::t_string;
use pp_common::AppInfo;

use crate::controller::ProjectController;
use crate::i18n::{use_i18n, Locale};
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::{AppState, Route};
use crate::theme::{apply_theme, get_pref, set_pref};
use crate::ui::Toasts;
use crate::view::board::BoardView;
use crate::view::imagery::ImageryView;
use crate::view::lab::LabView;
use crate::view::studio::StudioView;
use crate::view::placeholder::ComingSoon;
use crate::view::project_drawer::ProjectDrawer;
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
        state.cad_progress.set(Some((p.step, p.attempt, p.problems)));
    }) as Box<dyn FnMut(JsValue)>);
    spawn_local(async move {
        let _ = ipc::listen(ipc::event::CAD_PROGRESS, closure.into_js_value()).await;
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
    listen_cad_engine_progress(state);
    listen_image_progress(state);
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
                    Route::Settings => view! { <SettingsView state=state/> }.into_any(),
                    Route::Dashboard => view! {
                        <ComingSoon icon=IconKind::Dashboard title=move || t_string!(i18n, nav.dashboard)/>
                    }.into_any(),
                    Route::Ideas => view! {
                        <ComingSoon icon=IconKind::Lightbulb title=move || t_string!(i18n, nav.ideas)/>
                    }.into_any(),
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
        </div>
    }
}
