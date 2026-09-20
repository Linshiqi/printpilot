use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Brand,
    Green,
    Amber,
    Red,
}

#[component]
pub fn Badge(#[prop(optional)] tone: Tone, children: Children) -> impl IntoView {
    let color = match tone {
        Tone::Neutral => "bg-gray-100 text-gray-600 dark:bg-gray-700 dark:text-gray-300",
        Tone::Brand => "bg-brand-soft text-brand dark:bg-indigo-500/20 dark:text-indigo-300",
        Tone::Green => "bg-green-100 text-green-700 dark:bg-green-500/20 dark:text-green-300",
        Tone::Amber => "bg-amber-100 text-amber-700 dark:bg-amber-500/20 dark:text-amber-300",
        Tone::Red => "bg-red-100 text-red-700 dark:bg-red-500/20 dark:text-red-300",
    };
    view! {
        <span class=format!("inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[11px] font-medium leading-none {color}")>
            {children()}
        </span>
    }
}
