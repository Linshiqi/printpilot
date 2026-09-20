//! 技术预研页:三个面板,各验证一条技术主线。
//! - 3D 模型:three.js 视图桥 + 几何内核(预研 ① ②)
//! - 代码建模:图 → 设计规格 → build123d 脚本 → 真引擎执行;参数面板与指令修补(ADR-0003)
//! - 市场调研:供应商适配器 + 调研流水线(预研 ③)

mod cad;
mod model_panel;
mod research_panel;

use leptos::prelude::*;
use leptos_i18n::{t_string, td_string};

use crate::i18n::use_i18n;
use crate::state::AppState;
use crate::theme::{get_pref, set_pref};
use crate::ui::Segmented;
use cad::CadPanel;
use model_panel::ModelPanel;
use research_panel::ResearchPanel;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Model,
    Cad,
    Research,
}

impl Tab {
    fn key(self) -> &'static str {
        match self {
            Tab::Model => "model",
            Tab::Cad => "cad",
            Tab::Research => "research",
        }
    }
}

#[component]
pub fn LabView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let tab = RwSignal::new(match get_pref("lab_tab").as_deref() {
        Some("research") => Tab::Research,
        Some("cad") => Tab::Cad,
        _ => Tab::Model,
    });
    let label = move |f: fn(crate::i18n::Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center justify-between gap-4 border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, lab.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, lab.subtitle)}</p>
                </div>
                <Segmented
                    value=Signal::derive(move || tab.get())
                    options=vec![
                        (Tab::Model, label(|l| td_string!(l, lab.tab_model))),
                        (Tab::Cad, label(|l| td_string!(l, lab.tab_cad))),
                        (Tab::Research, label(|l| td_string!(l, lab.tab_research))),
                    ]
                    on_change=move |t: Tab| {
                        tab.set(t);
                        set_pref("lab_tab", t.key());
                    }
                />
            </header>
            <div class="flex-1 min-h-0">
                // 面板各管各的生命周期:切走 3D 面板时它会销毁 WebGL 上下文,切回来重新挂载
                {move || match tab.get() {
                    Tab::Model => view! { <ModelPanel state=state/> }.into_any(),
                    Tab::Cad => view! { <CadPanel state=state/> }.into_any(),
                    Tab::Research => view! { <ResearchPanel state=state/> }.into_any(),
                }}
            </div>
        </div>
    }
}
