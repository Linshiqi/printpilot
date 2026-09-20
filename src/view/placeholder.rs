use leptos::prelude::*;
use leptos::text_prop::TextProp;
use leptos_i18n::t_string;

use crate::i18n::use_i18n;
use crate::icon::IconKind;
use crate::ui::EmptyState;

/// 还没做的模块:老实说「后续里程碑交付」,不放假数据装样子。
#[component]
pub fn ComingSoon(icon: IconKind, #[prop(into)] title: TextProp) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col">
            <header class="px-6 h-14 flex items-center border-b border-gray-200 dark:border-gray-700">
                <h1 class="text-base font-semibold">{move || title.get().to_string()}</h1>
            </header>
            <div class="flex-1">
                <EmptyState
                    icon=icon
                    title=move || t_string!(i18n, common.coming_soon)
                    hint=move || t_string!(i18n, common.coming_soon_hint)
                />
            </div>
        </div>
    }
}
