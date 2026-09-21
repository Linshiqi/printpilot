//! 把文件拖进窗口时的提示。
//!
//! 本地文件的拖放走的是 Tauri 的原生事件(`app::listen_file_drops`),页面用 `state.accept_file_drops` 登记怎么接收;
//! 这里只管「悬着的时候告诉用户松手之后会发生什么」。整层 `pointer-events-none`——它只是提示,不参与落点的判断。

use leptos::prelude::*;

use crate::icon::{Icon, IconKind};
use crate::state::AppState;

/// 一块落点:虚线框 + 图标 + 一句话。`active` = 光标正在它上面、而且拖的东西它收。
#[component]
pub fn DropPanel(#[prop(into)] active: Signal<bool>, icon: IconKind, #[prop(into)] text: Signal<String>) -> impl IntoView {
    view! {
        <div
            // 底色要盖得住底下的字(空状态的说明、对话),否则两行字叠在一起谁也看不清
            class="h-full rounded-xl border-2 border-dashed backdrop-blur-sm flex flex-col items-center justify-center gap-2 text-sm transition-colors"
            class=("border-brand", move || active.get())
            class=("bg-brand-soft/95", move || active.get())
            class=("dark:bg-indigo-950/95", move || active.get())
            class=("text-brand", move || active.get())
            class=("dark:text-indigo-200", move || active.get())
            class=("border-gray-300", move || !active.get())
            class=("dark:border-gray-600", move || !active.get())
            class=("bg-white/85", move || !active.get())
            class=("dark:bg-gray-900/85", move || !active.get())
            class=("text-gray-400", move || !active.get())
        >
            <Icon kind=icon class="w-7 h-7"/>
            <span class="px-4 text-center font-medium">{move || text.get()}</span>
        </div>
    }
}

/// 整个页面只有一种去向时用的提示:盖满所在的(`relative`)容器。
/// `accepts`:拖着的这些文件里有没有这个页面收的;`text` / `reject`:收 / 不收时各说什么。
#[component]
pub fn FileDropHint(
    state: AppState,
    accepts: fn(&str) -> bool,
    icon: IconKind,
    #[prop(into)] text: Signal<String>,
    #[prop(into)] reject: Signal<String>,
) -> impl IntoView {
    // 悬着的时候光标每动一下 `file_drag` 都会变:收不收只跟路径有关,记下来,别每次都重算、重画
    let verdict = Memo::new(move |_| state.file_drag.with(|drag| drag.as_ref().map(|d| d.paths.iter().any(|p| accepts(p)))));
    view! {
        <Show when=move || verdict.get().is_some()>
            <div class="absolute inset-0 z-20 p-3 pointer-events-none">
                <DropPanel
                    active=Signal::derive(move || verdict.get() == Some(true))
                    icon=icon
                    text=Signal::derive(move || if verdict.get() == Some(false) { reject.get() } else { text.get() })
                />
            </div>
        </Show>
    }
}
