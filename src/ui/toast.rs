use leptos::prelude::*;

use crate::icon::{Icon, IconKind};
use crate::state::AppState;

/// 顶部居中的提示条:红 = 出错(6 秒,可手动关),绿 = 完成(2.5 秒)。由 `AppState::notify_*` 驱动。
/// 两条可能同时出现(导入一批图:几张成了、几张没成),所以放在同一个竖排的容器里,不互相盖住。
#[component]
pub fn Toasts(state: AppState) -> impl IntoView {
    view! {
        <div class="fixed top-3 left-1/2 -translate-x-1/2 z-[200] max-w-[80vw] flex flex-col items-center gap-2 pointer-events-none">
            <Show when=move || state.info_toast.get().is_some()>
                <div
                    class="pointer-events-auto flex items-start gap-2 bg-green-600 text-white text-xs px-4 py-2.5 rounded-lg shadow-lg animate-toast-in"
                    role="status"
                >
                    <Icon kind=IconKind::Check class="w-4 h-4 mt-0.5 shrink-0"/>
                    <span class="leading-snug break-words">{move || state.info_toast.get().unwrap_or_default()}</span>
                </div>
            </Show>
            <Show when=move || state.toast.get().is_some()>
                <div
                    class="pointer-events-auto flex items-start gap-2 bg-red-600 text-white text-xs px-4 py-2.5 rounded-lg shadow-lg selectable animate-toast-in"
                    role="alert"
                >
                    <Icon kind=IconKind::Alert class="w-4 h-4 mt-0.5 shrink-0"/>
                    <span class="leading-snug break-words">{move || state.toast.get().unwrap_or_default()}</span>
                    <button class="ml-2 text-white/80 hover:text-white" on:click=move |_| state.toast.set(None)>
                        <Icon kind=IconKind::Close class="w-3.5 h-3.5"/>
                    </button>
                </div>
            </Show>
        </div>
    }
}
