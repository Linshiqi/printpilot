//! 技术预研页:现在只剩 3D 模型工具(three.js 视图桥 + 几何内核,预研 ① ②)。
//! 毕业了的两条线各自成了一级入口:代码建模 → 「建模」(src/view/studio/),市场调研 → 「调研」(src/view/research/)。

mod model_panel;

use leptos::prelude::*;
use leptos_i18n::t_string;

use crate::i18n::use_i18n;
use crate::state::AppState;
use model_panel::ModelPanel;

#[component]
pub fn LabView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, lab.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, lab.subtitle)}</p>
                </div>
            </header>
            <div class="flex-1 min-h-0">
                <ModelPanel state=state/>
            </div>
        </div>
    }
}
