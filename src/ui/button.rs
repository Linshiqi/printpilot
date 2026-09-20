use leptos::prelude::*;
use leptos::text_prop::TextProp;

use crate::icon::{Icon, IconKind};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    Ghost,
    Danger,
}

const BASE: &str = "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium whitespace-nowrap \
                    transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50 \
                    disabled:opacity-50 disabled:pointer-events-none";

fn tone(variant: ButtonVariant) -> &'static str {
    match variant {
        ButtonVariant::Primary => "bg-brand text-white hover:bg-brand-dark",
        ButtonVariant::Secondary => {
            "border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-800 text-gray-700 dark:text-gray-200 \
             hover:bg-gray-50 dark:hover:bg-gray-700"
        }
        ButtonVariant::Ghost => "text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700/70",
        ButtonVariant::Danger => "bg-red-600 text-white hover:bg-red-700",
    }
}

#[component]
pub fn Button(
    #[prop(optional)] variant: ButtonVariant,
    #[prop(optional)] small: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] icon: Option<IconKind>,
    #[prop(into)] on_click: Callback<()>,
    children: Children,
) -> impl IntoView {
    let size = if small { "h-7 px-2.5 text-xs" } else { "h-9 px-3.5 text-sm" };
    view! {
        <button
            type="button"
            class=format!("{BASE} {size} {}", tone(variant))
            disabled=move || disabled.get()
            on:click=move |_| on_click.run(())
        >
            {icon.map(|kind| view! { <Icon kind=kind class="w-4 h-4 shrink-0"/> })}
            {children()}
        </button>
    }
}

/// 只有图标的按钮。`label` 既是悬停提示也是读屏文本。
#[component]
pub fn IconButton(
    icon: IconKind,
    #[prop(into)] label: TextProp,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    let title = label.clone();
    view! {
        <button
            type="button"
            class=format!("{BASE} h-8 w-8 {}", tone(ButtonVariant::Ghost))
            title=move || title.get().to_string()
            aria-label=move || label.get().to_string()
            disabled=move || disabled.get()
            on:click=move |_| on_click.run(())
        >
            <Icon kind=icon class="w-4 h-4"/>
        </button>
    }
}
