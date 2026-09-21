//! 项目看板:一张卡片 = 一个单品,列 = 阶段。拖动卡片推进阶段(docs/02-ux-flows.md §3.1)。

use leptos::prelude::*;
use leptos_i18n::t_string;
use pp_common::gate::gate_passed;
use pp_common::{NewProject, Project, ProjectStatus, Stage};
use wasm_bindgen::JsCast;

use crate::controller::{ForcedMove, ProjectController};
use crate::i18n::use_i18n;
use crate::i18n_util::{gate_label, stage_name, status_name};
use crate::icon::{Icon, IconKind};
use crate::pointer_drag::{PointerDrag, PointerDragGhost};
use crate::state::AppState;
use crate::ui::{Badge, Button, ButtonVariant, Dialog, DialogFooter, EmptyState, Field, TextArea, TextInput, Toggle, Tone};

const DROP_ATTR: &str = "data-drop-stage";

#[component]
pub fn BoardView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let ctl = ProjectController::new(state);
    let show_new = RwSignal::new(false);
    let show_finished = RwSignal::new(false);
    let forced = RwSignal::new(None::<ForcedMove>);

    let drag = PointerDrag::new(DROP_ATTR, false, move |project_id, stage_key| {
        if let Some(to) = Stage::parse(&stage_key) {
            ctl.request_move(project_id, to, forced);
        }
    });

    let is_empty = move || state.projects_loaded.get() && state.projects.with(Vec::is_empty);
    // 回到看板时重新数一遍各项目的产出:可能刚在工作台里出了图、建了模型
    state.reload_project_facts();

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center justify-between gap-4 border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, board.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, board.subtitle)}</p>
                </div>
                <div class="flex items-center gap-4">
                    <label class="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
                        <Toggle checked=show_finished on_change=move |v: bool| show_finished.set(v)/>
                        <span>
                            {move || format!(
                                "{} / {}",
                                t_string!(i18n, status.killed),
                                t_string!(i18n, status.done),
                            )}
                        </span>
                    </label>
                    <Button icon=IconKind::Plus on_click=move || show_new.set(true)>
                        {move || t_string!(i18n, board.new_project)}
                    </Button>
                </div>
            </header>

            <Show
                when=move || !is_empty()
                fallback=move || view! {
                    <EmptyState
                        icon=IconKind::Lightbulb
                        title=move || t_string!(i18n, board.empty_title)
                        hint=move || t_string!(i18n, board.empty_hint)
                    >
                        <Button icon=IconKind::Plus on_click=move || show_new.set(true)>
                            {move || t_string!(i18n, board.new_project)}
                        </Button>
                    </EmptyState>
                }
            >
                <div class="flex-1 min-h-0 overflow-x-auto">
                    <div class="h-full flex gap-3 p-4 min-w-max">
                        {Stage::ALL
                            .into_iter()
                            .map(|stage| view! { <Column state=state stage=stage drag=drag show_finished=show_finished/> })
                            .collect_view()}
                    </div>
                </div>
            </Show>

            <PointerDragGhost drag=drag icon=IconKind::Box/>
            <NewProjectDialog open=show_new ctl=ctl/>
            <ForcedMoveDialog pending=forced ctl=ctl/>
        </div>
    }
}

#[component]
fn Column(state: AppState, stage: Stage, drag: PointerDrag, show_finished: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let key = stage.as_str();
    let cards = move || {
        let finished = show_finished.get();
        state.projects.with(|list| {
            list.iter()
                .filter(|p| p.stage == stage)
                .filter(|p| finished || matches!(p.status, ProjectStatus::Active | ProjectStatus::Paused))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let over = move || drag.is_over(key);
    view! {
        <section
            data-drop-stage=key
            class="w-60 shrink-0 h-full flex flex-col rounded-xl border transition-colors"
            class=("border-brand", over)
            class=("bg-brand-soft", over)
            class=("dark:bg-indigo-500/10", over)
            class=("border-transparent", move || !over())
            class=("bg-gray-100", move || !over())
            class=("dark:bg-gray-800/60", move || !over())
        >
            <header class="shrink-0 flex items-center justify-between px-3 pt-3 pb-2">
                <span class="text-xs font-semibold text-gray-700 dark:text-gray-200">
                    {move || stage_name(i18n.get_locale(), stage)}
                </span>
                <span class="text-[11px] tabular-nums text-gray-400">{move || cards().len()}</span>
            </header>
            <div class="flex-1 min-h-0 overflow-y-auto px-2 pb-2 space-y-2">
                <For
                    each=cards
                    // 内容变了(改名、换状态、换阶段)键就变,卡片整张重建——卡片很轻,不值得做细粒度更新
                    key=|p| (p.id.clone(), p.updated_at, p.stage_entered_at)
                    let:project
                >
                    <ProjectCard state=state project=project drag=drag/>
                </For>
                <Show when=move || cards().is_empty()>
                    <div class="h-16 flex items-center justify-center rounded-lg border border-dashed border-gray-300 dark:border-gray-600 text-[11px] text-gray-400">
                        {move || t_string!(i18n, board.column_empty)}
                    </div>
                </Show>
            </div>
        </section>
    }
}

#[component]
fn ProjectCard(state: AppState, project: Project, drag: PointerDrag) -> impl IntoView {
    let i18n = use_i18n();
    let p = StoredValue::new(project.clone());
    let id = StoredValue::new(project.id.clone());
    let status = project.status;

    let days = move || p.with_value(|p| p.days_in_stage(state.now_ms.get()));
    let stale = move || p.with_value(|p| p.is_stale(state.now_ms.get()));
    let dragging = move || id.with_value(|id| drag.is_dragging(id));
    // 名下的产出(调研 / 图 / 模型)和「这个阶段的清单完成了没有」
    let facts = move || id.with_value(|id| state.facts_of(id));
    let stage = project.stage;
    let ready = move || status == ProjectStatus::Active && gate_passed(stage, &facts());

    view! {
        <article
            class="group rounded-lg border bg-white dark:bg-gray-800 p-3 space-y-2 cursor-grab active:cursor-grabbing shadow-sm hover:shadow transition-shadow"
            class=("border-amber-400", stale)
            class=("border-gray-200", move || !stale())
            class=("dark:border-gray-700", move || !stale())
            class=("opacity-40", dragging)
            on:pointerdown=move |ev| {
                let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else {
                    return;
                };
                let (payload, label) = p.with_value(|p| (p.id.clone(), format!("{} {}", p.code, p.title)));
                drag.press(&ev, el, payload, label);
            }
            on:click=move |_| {
                // 拖拽结束时浏览器还会补发一次 click,要吞掉,否则每次拖完都会弹出详情
                if !PointerDrag::swallow_click() {
                    state.open_project.set(Some(id.get_value()));
                }
            }
        >
            <div class="flex items-center justify-between gap-2">
                <span class="text-[11px] font-mono text-gray-400">{project.code.clone()}</span>
                {(status != ProjectStatus::Active).then(|| {
                    let tone = match status {
                        ProjectStatus::Paused => Tone::Amber,
                        ProjectStatus::Killed => Tone::Red,
                        ProjectStatus::Done => Tone::Green,
                        ProjectStatus::Active => Tone::Neutral,
                    };
                    view! { <Badge tone=tone>{move || status_name(i18n.get_locale(), status)}</Badge> }
                })}
            </div>
            <h3 class="text-sm font-medium leading-snug text-gray-900 dark:text-gray-50 break-words">
                {project.title.clone()}
            </h3>
            {(!project.category.is_empty()).then(|| view! {
                <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{project.category.clone()}</p>
            })}
            <div
                class="flex items-center gap-1 text-[11px]"
                class=("text-amber-600", stale)
                class=("dark:text-amber-400", stale)
                class=("text-gray-400", move || !stale())
            >
                <Icon kind=IconKind::Clock class="w-3 h-3"/>
                <span>
                    {move || match (days(), stale()) {
                        (0, _) => t_string!(i18n, board.today).to_string(),
                        (d, true) => t_string!(i18n, board.stale, days = d).to_string(),
                        (d, false) => t_string!(i18n, board.days_in_stage, days = d).to_string(),
                    }}
                </span>
            </div>
            // 三个工作台在这个项目名下各产出了什么;清单完成时提示可以推进
            <div class="flex items-center gap-2.5 text-[11px] tabular-nums text-gray-400">
                {move || {
                    let f = facts();
                    let cell = |icon: IconKind, n: u32, title: String| (n > 0).then(|| view! {
                        <span class="inline-flex items-center gap-0.5" title=title>
                            <Icon kind=icon class="w-3 h-3"/>
                            {n}
                        </span>
                    });
                    view! {
                        {cell(IconKind::Lightbulb, f.research_runs, t_string!(i18n, project.work_research).to_string())}
                        {cell(IconKind::Image, f.images, t_string!(i18n, project.work_images).to_string())}
                        {cell(IconKind::Box, f.models, t_string!(i18n, project.work_models).to_string())}
                    }
                }}
                <div class="flex-1"></div>
                <Show when=ready>
                    <span class="inline-flex items-center gap-0.5 font-medium text-green-600 dark:text-green-400">
                        <Icon kind=IconKind::Check class="w-3 h-3"/>
                        {move || t_string!(i18n, project.ready_badge)}
                    </span>
                </Show>
            </div>
        </article>
    }
}

#[component]
fn NewProjectDialog(open: RwSignal<bool>, ctl: ProjectController) -> impl IntoView {
    let i18n = use_i18n();
    let title = RwSignal::new(String::new());
    let category = RwSignal::new(String::new());
    let hypothesis = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    // 每次打开都是一张白纸
    Effect::new(move |_| {
        if open.get() {
            title.set(String::new());
            category.set(String::new());
            hypothesis.set(String::new());
            busy.set(false);
        }
    });

    let can_submit = Signal::derive(move || !busy.get() && !title.with(|t| t.trim().is_empty()));
    let submit = move || {
        if !can_submit.get_untracked() {
            return;
        }
        busy.set(true);
        ctl.create(
            NewProject {
                title: title.get_untracked(),
                category: category.get_untracked(),
                hypothesis: hypothesis.get_untracked(),
            },
            move || open.set(false),
        );
        // 失败时对话框留着、toast 报错;按钮要能再点
        set_timeout(move || busy.set(false), std::time::Duration::from_millis(800));
    };

    view! {
        <Dialog open=open title=move || t_string!(i18n, project.new_title)>
            <Field label=move || t_string!(i18n, project.title_label)>
                <TextInput
                    value=title
                    autofocus=true
                    placeholder=move || t_string!(i18n, project.title_placeholder)
                    on_enter=submit
                />
            </Field>
            <Field label=move || t_string!(i18n, project.category_label)>
                <TextInput value=category placeholder=move || t_string!(i18n, project.category_placeholder)/>
            </Field>
            <Field
                label=move || t_string!(i18n, project.hypothesis_label)
                hint=move || t_string!(i18n, project.hypothesis_hint)
            >
                <TextArea value=hypothesis placeholder=move || t_string!(i18n, project.hypothesis_placeholder)/>
            </Field>
            <DialogFooter>
                <Button variant=ButtonVariant::Ghost on_click=move || open.set(false)>
                    {move || t_string!(i18n, common.cancel)}
                </Button>
                <Button disabled=Signal::derive(move || !can_submit.get()) on_click=submit>
                    {move || t_string!(i18n, common.create)}
                </Button>
            </DialogFooter>
        </Dialog>
    }
}

/// 往前推进但证据还不够(清单没完成 / 整个跳过了一个阶段):补一句原因再放行
/// (docs/01-prd.md §4「可强行推进,但必须填写原因」;规则在 pp_common::gate)。看板拖拽和项目中枢共用。
#[component]
pub fn ForcedMoveDialog(pending: RwSignal<Option<ForcedMove>>, ctl: ProjectController) -> impl IntoView {
    let i18n = use_i18n();
    let open = RwSignal::new(false);
    let reason = RwSignal::new(String::new());

    Effect::new(move |_| {
        if pending.with(Option::is_some) {
            reason.set(String::new());
            open.set(true);
        }
    });
    // 关掉对话框 = 放弃这次移动(卡片本来就没动)
    Effect::new(move |_| {
        if !open.get() {
            pending.set(None);
        }
    });

    let can_submit = Signal::derive(move || !reason.with(|r| r.trim().is_empty()));
    let submit = move || {
        let Some(m) = pending.get_untracked() else { return };
        if !can_submit.get_untracked() {
            return;
        }
        ctl.move_stage(m.project_id, m.to, reason.get_untracked());
        open.set(false);
    };

    view! {
        <Dialog open=open title=move || t_string!(i18n, project.forced_title)>
            <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">
                {move || pending.get().map(|m| {
                    let l = i18n.get_locale();
                    format!(
                        "{} · {}",
                        m.code,
                        t_string!(i18n, project.forced_hint, from = stage_name(l, m.from), to = stage_name(l, m.to)),
                    )
                })}
            </p>
            // 到底缺什么,说清楚
            {move || pending.get().map(|m| {
                let l = i18n.get_locale();
                if m.unmet.is_empty() {
                    return view! { <p class="text-xs text-gray-500 dark:text-gray-400">{move || t_string!(i18n, project.forced_skipped)}</p> }.into_any();
                }
                view! {
                    <ul class="space-y-1 text-xs text-amber-700 dark:text-amber-300">
                        {m.unmet.iter().map(|(stage, key)| view! {
                            <li>{format!("· {}:{}", stage_name(l, *stage), gate_label(l, *key))}</li>
                        }).collect_view()}
                    </ul>
                }.into_any()
            })}
            <Field label=move || t_string!(i18n, project.forced_reason_label)>
                <TextInput
                    value=reason
                    autofocus=true
                    placeholder=move || t_string!(i18n, project.forced_reason_placeholder)
                    on_enter=submit
                />
            </Field>
            <DialogFooter>
                <Button variant=ButtonVariant::Ghost on_click=move || open.set(false)>
                    {move || t_string!(i18n, common.cancel)}
                </Button>
                <Button disabled=Signal::derive(move || !can_submit.get()) on_click=submit>
                    {move || t_string!(i18n, common.confirm)}
                </Button>
            </DialogFooter>
        </Dialog>
    }
}
