use leptos::ev;
use leptos::prelude::*;
use leptos::text_prop::TextProp;

use super::button::IconButton;
use crate::icon::IconKind;
use crate::i18n::use_i18n;
use leptos_i18n::t_string;

/// Esc 关闭:在 window 上挂一个 keydown 监听,组件销毁时摘掉。
fn close_on_escape(open: RwSignal<bool>) {
    let handle = window_event_listener(ev::keydown, move |e| {
        if e.key() == "Escape" && open.get_untracked() {
            open.set(false);
        }
    });
    on_cleanup(move || handle.remove());
}

/// 点到遮罩本身(而不是里面的卡片)才关闭。用 mousedown 而不是 click:
/// 在输入框里按下、拖到遮罩上松开,不应该把对话框关掉。
fn backdrop_pressed(e: &web_sys::MouseEvent) -> bool {
    e.target() == e.current_target()
}

/// 居中的模态对话框。按钮行用 `DialogFooter` 放在 children 末尾。
#[component]
pub fn Dialog(
    open: RwSignal<bool>,
    #[prop(into)] title: TextProp,
    #[prop(optional)] wide: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let i18n = use_i18n();
    close_on_escape(open);
    let title = StoredValue::new(title);
    let children = StoredValue::new(children);
    let width = if wide { "max-w-2xl" } else { "max-w-md" };
    view! {
        <Show when=move || open.get()>
            <div
                class="fixed inset-0 z-[110] flex items-center justify-center bg-black/40 p-6"
                on:mousedown=move |e| {
                    if backdrop_pressed(&e) {
                        open.set(false);
                    }
                }
            >
                <div
                    role="dialog"
                    aria-modal="true"
                    class=format!(
                        "w-full {width} max-h-full flex flex-col rounded-2xl bg-white dark:bg-gray-800 shadow-2xl \
                         border border-gray-200 dark:border-gray-700 animate-card-in"
                    )
                >
                    <header class="flex items-center justify-between gap-3 px-5 pt-4 pb-2">
                        <h2 class="text-base font-semibold text-gray-900 dark:text-gray-50">
                            {move || title.get_value().get().to_string()}
                        </h2>
                        <IconButton
                            icon=IconKind::Close
                            label=move || t_string!(i18n, common.close).to_string()
                            on_click=move || open.set(false)
                        />
                    </header>
                    <div class="px-5 pb-5 pt-2 space-y-4 overflow-y-auto">{children.get_value()()}</div>
                </div>
            </div>
        </Show>
    }
}

#[component]
pub fn DialogFooter(children: Children) -> impl IntoView {
    view! { <div class="flex items-center justify-end gap-2 pt-2">{children()}</div> }
}

/// 右侧滑出的抽屉:看详情、不打断当前页面。
#[component]
pub fn Drawer(
    open: RwSignal<bool>,
    #[prop(into)] title: TextProp,
    /// 宽一档(项目中枢要摆清单和缩略图)
    #[prop(optional)]
    wide: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let i18n = use_i18n();
    close_on_escape(open);
    let title = StoredValue::new(title);
    let children = StoredValue::new(children);
    view! {
        <Show when=move || open.get()>
            <div
                class="fixed inset-0 z-[100] flex justify-end bg-black/30"
                on:mousedown=move |e| {
                    if backdrop_pressed(&e) {
                        open.set(false);
                    }
                }
            >
                <aside
                    role="dialog"
                    aria-modal="true"
                    class="max-w-full h-full flex flex-col bg-white dark:bg-gray-800 shadow-2xl \
                           border-l border-gray-200 dark:border-gray-700 animate-drawer-in"
                    class=("w-[440px]", !wide)
                    class=("w-[600px]", wide)
                >
                    <header class="flex items-center justify-between gap-3 px-5 py-3 border-b border-gray-200 dark:border-gray-700">
                        <h2 class="text-base font-semibold text-gray-900 dark:text-gray-50 truncate">
                            {move || title.get_value().get().to_string()}
                        </h2>
                        <IconButton
                            icon=IconKind::Close
                            label=move || t_string!(i18n, common.close).to_string()
                            on_click=move || open.set(false)
                        />
                    </header>
                    <div class="flex-1 overflow-y-auto p-5 space-y-5">{children.get_value()()}</div>
                </aside>
            </div>
        </Show>
    }
}
