use leptos::prelude::*;
use leptos::text_prop::TextProp;

#[component]
pub fn Card(#[prop(optional)] class: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class=format!(
            "rounded-xl border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 {class}"
        )>{children()}</section>
    }
}

#[component]
pub fn SectionTitle(#[prop(into)] title: TextProp, #[prop(optional, into)] hint: Option<TextProp>) -> impl IntoView {
    view! {
        <div class="space-y-0.5">
            <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-100">{move || title.get().to_string()}</h2>
            {hint.map(|h| view! { <p class="text-xs text-gray-500 dark:text-gray-400">{move || h.get().to_string()}</p> })}
        </div>
    }
}
