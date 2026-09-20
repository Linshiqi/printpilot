//! 全局状态(做法照搬 velo):一个 `Copy` 的扁平结构体,字段全是 `RwSignal`。
//! `App` 里 `provide_context` 一次,各处 `use_context::<AppState>()`;因为是 `Copy`,闭包里直接 move。
//!
//! 协作约定:view 只读信号、只调 controller 方法;controller 方法里 `spawn_local` → `ipc::call` → 写回信号。

use leptos::prelude::*;
use pp_common::provider::ProviderStatus;
use pp_common::{AppInfo, Project};

use crate::theme::Theme;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Dashboard,
    Projects,
    Ideas,
    Calendar,
    Orders,
    Analytics,
    Settings,
    Lab,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Dashboard => "dashboard",
            Route::Projects => "projects",
            Route::Ideas => "ideas",
            Route::Calendar => "calendar",
            Route::Orders => "orders",
            Route::Analytics => "analytics",
            Route::Settings => "settings",
            Route::Lab => "lab",
        }
    }

    pub fn parse(s: &str) -> Option<Route> {
        [
            Route::Dashboard,
            Route::Projects,
            Route::Ideas,
            Route::Calendar,
            Route::Orders,
            Route::Analytics,
            Route::Settings,
            Route::Lab,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }
}

#[derive(Copy, Clone)]
pub struct AppState {
    pub route: RwSignal<Route>,
    pub theme: RwSignal<Theme>,
    /// 错误提示(红)与普通提示(绿),各自定时消失
    pub toast: RwSignal<Option<String>>,
    pub info_toast: RwSignal<Option<String>>,
    pub app_info: RwSignal<Option<AppInfo>>,
    pub projects: RwSignal<Vec<Project>>,
    pub projects_loaded: RwSignal<bool>,
    /// 右侧抽屉里打开的项目
    pub open_project: RwSignal<Option<String>>,
    /// 「现在」的毫秒时间戳,每分钟刷新一次:看板上的「已停留 N 天」靠它更新
    pub now_ms: RwSignal<i64>,
    /// 各供应商有没有配密钥(只有「有 / 没有」,密钥本身永远不到前端)
    pub providers: RwSignal<Vec<ProviderStatus>>,
    /// 正在运行的调研走到了哪一步:`(step, done, total)`,由后端的 research-progress 事件驱动
    pub research_progress: RwSignal<Option<(String, u32, u32)>>,
    /// 正在运行的建模走到了哪一步:`(step, attempt, problems)`,由后端的 cad-progress 事件驱动
    pub cad_progress: RwSignal<Option<(String, u32, u32)>>,
    /// 引擎包解包进度:`(phase, done, total)`,由 cad-engine-progress 事件驱动
    pub cad_engine_progress: RwSignal<Option<(String, u64, u64)>>,
}

impl AppState {
    pub fn new() -> Self {
        // 上次停留的页面(界面偏好,存 localStorage)
        let route = crate::theme::get_pref("route")
            .and_then(|s| Route::parse(&s))
            .unwrap_or(Route::Projects);
        Self {
            route: RwSignal::new(route),
            theme: RwSignal::new(Theme::init()),
            toast: RwSignal::new(None),
            info_toast: RwSignal::new(None),
            app_info: RwSignal::new(None),
            projects: RwSignal::new(Vec::new()),
            projects_loaded: RwSignal::new(false),
            open_project: RwSignal::new(None),
            now_ms: RwSignal::new(crate::utils::now_ms()),
            providers: RwSignal::new(Vec::new()),
            research_progress: RwSignal::new(None),
            cad_progress: RwSignal::new(None),
            cad_engine_progress: RwSignal::new(None),
        }
    }

    /// 重新读一遍各供应商的密钥状态(启动时、保存或删除密钥之后)。
    pub fn reload_providers(self) {
        leptos::task::spawn_local(async move {
            match crate::ipc::call_no_args::<Vec<ProviderStatus>>(crate::ipc::cmd::PROVIDER_STATUS).await {
                Ok(list) => self.providers.set(list),
                Err(e) => self.notify_error(e),
            }
        });
    }

    pub fn notify_error(self, msg: impl Into<String>) {
        let text = msg.into();
        web_sys::console::error_1(&text.clone().into());
        self.toast.set(Some(text));
        let toast = self.toast;
        set_timeout(move || toast.set(None), std::time::Duration::from_millis(6000));
    }

    pub fn notify_info(self, msg: impl Into<String>) {
        self.info_toast.set(Some(msg.into()));
        let toast = self.info_toast;
        set_timeout(move || toast.set(None), std::time::Duration::from_millis(2500));
    }

    pub fn go(self, route: Route) {
        self.route.set(route);
        crate::theme::set_pref("route", route.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_strings_roundtrip() {
        for r in [
            Route::Dashboard,
            Route::Projects,
            Route::Ideas,
            Route::Calendar,
            Route::Orders,
            Route::Analytics,
            Route::Settings,
            Route::Lab,
        ] {
            assert_eq!(Route::parse(r.as_str()), Some(r));
        }
        assert_eq!(Route::parse("nowhere"), None);
    }
}
