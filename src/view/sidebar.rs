use leptos::prelude::*;
use leptos::text_prop::TextProp;
use leptos_i18n::t_string;

use crate::i18n::use_i18n;
use crate::icon::{Icon, IconKind};
use crate::state::{AppState, Route};
use crate::theme::Theme;
use crate::ui::IconButton;

#[component]
fn NavItem(state: AppState, route: Route, icon: IconKind, #[prop(into)] label: TextProp) -> impl IntoView {
    let active = move || state.route.get() == route;
    view! {
        <button
            type="button"
            class="w-full flex items-center gap-2.5 h-9 px-3 rounded-lg text-sm transition-colors"
            class=("bg-brand-soft", active)
            class=("text-brand", active)
            class=("font-medium", active)
            class=("dark:bg-indigo-500/15", active)
            class=("dark:text-indigo-300", active)
            class=("text-gray-600", move || !active())
            class=("dark:text-gray-300", move || !active())
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| state.go(route)
        >
            <Icon kind=icon class="w-4 h-4 shrink-0"/>
            <span class="truncate">{move || label.get().to_string()}</span>
        </button>
    }
}

#[component]
pub fn Sidebar(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <nav class="w-52 shrink-0 h-full flex flex-col border-r border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800">
            <div class="flex items-center gap-2.5 px-4 h-14">
                <div class="w-7 h-7 rounded-lg bg-brand text-white flex items-center justify-center">
                    <Icon kind=IconKind::Layers class="w-4 h-4"/>
                </div>
                <div class="leading-tight min-w-0">
                    <div class="text-sm font-semibold text-gray-900 dark:text-gray-50">"PrintPilot"</div>
                    <div class="text-[11px] text-gray-400 truncate">{move || t_string!(i18n, app.tagline)}</div>
                </div>
            </div>
            <div class="flex-1 overflow-y-auto px-2 py-2 space-y-0.5">
                <NavItem state=state route=Route::Dashboard icon=IconKind::Dashboard label=move || t_string!(i18n, nav.dashboard)/>
                <NavItem state=state route=Route::Projects icon=IconKind::Kanban label=move || t_string!(i18n, nav.projects)/>
                // 三个工作台,按一个单品走过的顺序排:调研 → 图片 → 建模
                <NavItem state=state route=Route::Ideas icon=IconKind::Lightbulb label=move || t_string!(i18n, nav.ideas)/>
                <NavItem state=state route=Route::Images icon=IconKind::Image label=move || t_string!(i18n, nav.images)/>
                <NavItem state=state route=Route::Studio icon=IconKind::Box label=move || t_string!(i18n, nav.studio)/>
                <NavItem state=state route=Route::Calendar icon=IconKind::Calendar label=move || t_string!(i18n, nav.calendar)/>
                <NavItem state=state route=Route::Orders icon=IconKind::Package label=move || t_string!(i18n, nav.orders)/>
                <NavItem state=state route=Route::Analytics icon=IconKind::Chart label=move || t_string!(i18n, nav.analytics)/>
            </div>
            <div class="px-2 py-2 space-y-0.5 border-t border-gray-200 dark:border-gray-700">
                <NavItem state=state route=Route::Lab icon=IconKind::Flask label=move || t_string!(i18n, nav.lab)/>
                <NavItem state=state route=Route::Settings icon=IconKind::Settings label=move || t_string!(i18n, nav.settings)/>
                <div class="flex items-center justify-between pl-3 pr-1 pt-1">
                    <span class="text-[11px] text-gray-400 tabular-nums">
                        {move || state.app_info.with(|i| i.as_ref().map(|i| format!("v{}", i.version)).unwrap_or_default())}
                    </span>
                    // 图标表示「点了之后会变成什么」:浅色下显示月亮
                    {move || {
                        let (icon, label) = match state.theme.get() {
                            Theme::Light => (IconKind::Moon, t_string!(i18n, common.theme_dark)),
                            Theme::Dark => (IconKind::Sun, t_string!(i18n, common.theme_light)),
                        };
                        view! { <IconButton icon=icon label=label on_click=move || state.theme.update(|t| *t = t.toggle())/> }
                    }}
                </div>
            </div>
        </nav>
    }
}
