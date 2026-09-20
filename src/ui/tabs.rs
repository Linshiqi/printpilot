use leptos::prelude::*;

/// 分段选择器:几个互斥选项排成一行(主题、语言、样例形状…)。
/// `options` 是 `(值, 显示文本)`;文本用闭包给,语言切换时才会跟着变。
#[component]
pub fn Segmented<T>(value: Signal<T>, options: Vec<(T, Signal<String>)>, #[prop(into)] on_change: Callback<(T,)>) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    view! {
        <div class="inline-flex p-0.5 rounded-lg bg-gray-100 dark:bg-gray-700/60">
            {options
                .into_iter()
                .map(|(opt, label)| {
                    view! {
                        <button
                            type="button"
                            class="h-7 px-3 rounded-md text-xs font-medium transition-colors"
                            class=("bg-white", move || value.get() == opt)
                            class=("dark:bg-gray-900", move || value.get() == opt)
                            class=("shadow-sm", move || value.get() == opt)
                            class=("text-gray-900", move || value.get() == opt)
                            class=("dark:text-gray-50", move || value.get() == opt)
                            class=("text-gray-500", move || value.get() != opt)
                            class=("dark:text-gray-400", move || value.get() != opt)
                            on:click=move |_| on_change.run((opt,))
                        >
                            {move || label.get()}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}
