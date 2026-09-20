use leptos::prelude::*;
use leptos::text_prop::TextProp;

const INPUT: &str = "w-full px-3 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 \
                     text-sm text-gray-900 dark:text-gray-100 placeholder:text-gray-400 \
                     focus:outline-none focus:ring-2 focus:ring-brand/40 focus:border-brand";

/// 表单项外壳:标签 + 控件 + 可选的说明文字。
#[component]
pub fn Field(
    #[prop(into)] label: TextProp,
    #[prop(optional, into)] hint: Option<TextProp>,
    children: Children,
) -> impl IntoView {
    view! {
        <label class="block space-y-1.5">
            <span class="block text-xs font-medium text-gray-600 dark:text-gray-300">
                {move || label.get().to_string()}
            </span>
            {children()}
            {hint.map(|h| view! { <span class="block text-xs text-gray-400">{move || h.get().to_string()}</span> })}
        </label>
    }
}

fn placeholder_text(p: &Option<TextProp>) -> String {
    p.as_ref().map(|p| p.get().to_string()).unwrap_or_default()
}

#[component]
pub fn TextInput(
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: Option<TextProp>,
    /// 挂载后自动聚焦(对话框里的第一个输入框)
    #[prop(optional)]
    autofocus: bool,
    /// 数字输入:只影响软键盘/输入法提示,值仍然是字符串,用 `utils::parse_positive` 解析
    #[prop(optional)]
    numeric: bool,
    /// 密钥之类的输入:不回显
    #[prop(optional)]
    password: bool,
    #[prop(optional, into)] on_enter: Option<Callback<()>>,
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Input>::new();
    if autofocus {
        Effect::new(move |_| {
            if let Some(el) = node.get() {
                let _ = el.focus();
            }
        });
    }
    view! {
        <input
            node_ref=node
            type=if password { "password" } else { "text" }
            autocomplete="off"
            spellcheck="false"
            inputmode=if numeric { "decimal" } else { "text" }
            class=format!("{INPUT} h-9")
            placeholder=move || placeholder_text(&placeholder)
            prop:value=move || value.get()
            on:input=move |e| value.set(event_target_value(&e))
            on:keydown=move |e| {
                // 输入法组合中的回车是「确认候选词」,不是提交
                if e.key() == "Enter" && !e.is_composing() {
                    if let Some(cb) = on_enter {
                        cb.run(());
                    }
                }
            }
        />
    }
}

#[component]
pub fn TextArea(
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: Option<TextProp>,
    #[prop(default = 3)] rows: u32,
) -> impl IntoView {
    view! {
        <textarea
            rows=rows
            class=format!("{INPUT} py-2 leading-relaxed resize-none")
            placeholder=move || placeholder_text(&placeholder)
            prop:value=move || value.get()
            on:input=move |e| value.set(event_target_value(&e))
        ></textarea>
    }
}

/// 开关。`on_change` 拿到的是切换后的值;`checked` 由调用方在保存成功后再更新,
/// 这样保存失败时界面不会停在一个假状态上。
#[component]
pub fn Toggle(
    #[prop(into)] checked: Signal<bool>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(into)] on_change: Callback<(bool,)>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            role="switch"
            aria-checked=move || checked.get().to_string()
            disabled=move || disabled.get()
            class="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors \
                   disabled:opacity-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-brand/50"
            class=("bg-brand", move || checked.get())
            class=("bg-gray-300", move || !checked.get())
            class=("dark:bg-gray-600", move || !checked.get())
            on:click=move |_| on_change.run((!checked.get_untracked(),))
        >
            <span
                class="inline-block h-4 w-4 rounded-full bg-white shadow transition-transform"
                class=("translate-x-4", move || checked.get())
                class=("translate-x-0.5", move || !checked.get())
            ></span>
        </button>
    }
}
