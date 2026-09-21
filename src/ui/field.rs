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

fn parse_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

fn format_number(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// 数字输入,直接绑一个 `f64` 信号(成本定价器里有二十来个这样的格子)。
/// - `scale`:显示值 = 信号值 × scale。百分比用 100:信号里是 0.05,格子里显示 5;
/// - 每敲一个能解析的数就写回信号(定价页靠它「改任一参数即时重算」);敲到一半的(空、`1.`)不写;
/// - 外面改了信号(带入档案、用实际值)→ 格子跟着变;自己输入引起的变化不回写,否则光标会跳;
/// - 失焦时把格子里的字规整成信号里的值。
///
/// ⚠️ 信号 ↔ 文字是两个方向的同步:都必须「不一样才写」,见 CLAUDE.md 里那条互相镜像的 Effect 的坑。
#[component]
pub fn NumInput(
    value: RwSignal<f64>,
    #[prop(default = 1.0)] scale: f64,
    #[prop(default = 2)] decimals: usize,
    /// 格子右边的单位(g、h、¥、% …)
    #[prop(optional, into)]
    unit: Option<TextProp>,
) -> impl IntoView {
    let shown = move |v: f64| format_number(v * scale, decimals);
    let text = RwSignal::new(shown(value.get_untracked()));
    Effect::new(move |_| {
        let v = value.get();
        let same = text.with_untracked(|t| parse_number(t)).is_some_and(|typed| (typed / scale - v).abs() < 1e-9);
        if !same {
            text.set(shown(v));
        }
    });
    view! {
        <div class="relative">
            <input
                type="text"
                inputmode="decimal"
                autocomplete="off"
                spellcheck="false"
                class=format!("{INPUT} h-8 tabular-nums {}", if unit.is_some() { "pr-9" } else { "" })
                prop:value=move || text.get()
                on:input=move |e| {
                    let raw = event_target_value(&e);
                    if let (Some(typed), Some(current)) = (parse_number(&raw), value.try_get_untracked()) {
                        let v = typed / scale;
                        if (v - current).abs() > 1e-12 {
                            value.set(v);
                        }
                    }
                    let _ = text.try_set(raw);
                }
                on:blur=move |_| {
                    // 对话框 / 页面关掉时,正聚焦的格子会在被移除的瞬间收到 blur——那时信号已经随作用域销毁了,
                    // 直接 get 会 panic。所以这里用 try_ 系列:信号没了就什么都不做
                    if let Some(v) = value.try_get_untracked() {
                        let _ = text.try_set(shown(v));
                    }
                }
            />
            {unit.map(|u| view! {
                <span class="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-xs text-gray-400">{move || u.get().to_string()}</span>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_shown_without_trailing_zeros_and_parsed_leniently() {
        assert_eq!(format_number(27.0, 2), "27");
        assert_eq!(format_number(1.70, 2), "1.7");
        assert_eq!(format_number(0.05 * 100.0, 2), "5");
        assert_eq!(format_number(29.9, 2), "29.9");
        assert_eq!(format_number(100.0, 0), "100", "没有小数点时不能把整数末尾的 0 删掉");
        assert_eq!(parse_number(" 1. "), Some(1.0), "敲到一半的 1. 也算数");
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("-3"), None, "成本里没有负数");
        assert_eq!(parse_number("abc"), None);
    }
}
