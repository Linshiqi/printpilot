use leptos::prelude::*;
use leptos::text_prop::TextProp;

use crate::icon::{Icon, IconKind};

/// 空状态:一句话说明 + 「下一步该做什么」的单一主按钮(放在 children 里),不放长文。
#[component]
pub fn EmptyState(
    icon: IconKind,
    #[prop(into)] title: TextProp,
    #[prop(optional, into)] hint: Option<TextProp>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="h-full flex flex-col items-center justify-center text-center gap-3 p-10">
            <div class="w-12 h-12 rounded-2xl bg-brand-soft dark:bg-indigo-500/15 text-brand dark:text-indigo-300 flex items-center justify-center">
                <Icon kind=icon class="w-6 h-6"/>
            </div>
            <h3 class="text-sm font-semibold text-gray-800 dark:text-gray-100">{move || title.get().to_string()}</h3>
            {hint.map(|h| view! {
                <p class="max-w-sm text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || h.get().to_string()}</p>
            })}
            {children.map(|c| view! { <div class="pt-1">{c()}</div> })}
        </div>
    }
}
