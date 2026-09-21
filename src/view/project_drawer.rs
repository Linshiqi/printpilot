//! 项目中枢(右侧抽屉):项目是主线,三个工作台(调研 / 图片 / 建模)的产出都挂在这里。
//!
//!   阶段条 → 「下一步」卡(这个阶段的清单 + 该去哪个工作台 + 进入下一阶段)→ 名下的调研 / 图片 / 模型
//!   → 基本信息 → 阶段流转时间线 → 暂停 / 淘汰 / 删除
//!
//! 清单不是手工打勾的:它从名下的产出里算出来(pp_common::gate)。
//! 打样之后的阶段(成本、上架、订单、数据)的工具还没做出来,到那里清单是空的,靠人判断。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::t_string;
use pp_common::cost::PricingSummary;
use pp_common::gate::{gate_passed, stage_gate, ProjectOverview};
use pp_common::{Project, ProjectStatus, Stage, StageEvent};

use crate::controller::{ForcedMove, ProjectController};
use crate::i18n::use_i18n;
use crate::i18n_util::{gate_label, stage_name, status_name};
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::ui::{Badge, Button, ButtonVariant, Dialog, DialogFooter, Drawer, Field, SectionTitle, TextArea, TextInput, Tone};
use crate::utils::{fmt_mm, format_ts, local_tz_offset_minutes};
use crate::view::board::ForcedMoveDialog;

#[component]
pub fn ProjectDrawer(state: AppState) -> impl IntoView {
    let open = RwSignal::new(false);
    // 两个方向的同步:`open_project` 有值 ⇔ 抽屉开着(从工作台跳走时控制器会把它清掉,抽屉跟着关)。
    // ⚠️ `set` 不管值变没变都会通知订阅者:两边都必须「不一样才写」,否则这两个 Effect 会互相触发、把界面线程卡死。
    Effect::new(move |_| {
        let want = state.open_project.with(Option::is_some);
        if open.get_untracked() != want {
            open.set(want);
        }
    });
    Effect::new(move |_| {
        if !open.get() && state.open_project.with_untracked(Option::is_some) {
            state.open_project.set(None);
        }
    });

    let title = move || {
        let id = state.open_project.get();
        state
            .projects
            .with(|list| list.iter().find(|p| Some(&p.id) == id.as_ref()).map(|p| format!("{} · {}", p.code, p.title)))
            .unwrap_or_default()
    };

    view! {
        <Drawer open=open title=title wide=true>
            // 只跟踪「打开的是哪个项目」:项目列表刷新时不重建表单,用户没保存的输入不会被冲掉
            {move || state.open_project.get().map(|id| view! { <ProjectDetail state=state project_id=id/> })}
        </Drawer>
    }
}

#[component]
fn ProjectDetail(state: AppState, project_id: String) -> impl IntoView {
    let i18n = use_i18n();
    let ctl = ProjectController::new(state);
    let id = StoredValue::new(project_id.clone());

    let find = move |list: &Vec<Project>| id.with_value(|id| list.iter().find(|p| &p.id == id).cloned());
    let initial = state.projects.with_untracked(find);
    let project = Memo::new(move |_| state.projects.with(find));

    let title = RwSignal::new(initial.as_ref().map(|p| p.title.clone()).unwrap_or_default());
    let category = RwSignal::new(initial.as_ref().map(|p| p.category.clone()).unwrap_or_default());
    let hypothesis = RwSignal::new(initial.as_ref().map(|p| p.hypothesis.clone()).unwrap_or_default());

    let dirty = Signal::derive(move || {
        project.with(|p| {
            p.as_ref().is_some_and(|p| {
                title.with(|t| t.trim() != p.title)
                    || category.with(|c| c.trim() != p.category)
                    || hypothesis.with(|h| h.trim() != p.hypothesis)
            })
        })
    });
    let can_save = Signal::derive(move || dirty.get() && !title.with(|t| t.trim().is_empty()));

    // 时间线:打开时取一次;阶段变了(拖拽或这里的操作)再取
    let events = RwSignal::new(Vec::<StageEvent>::new());
    Effect::new(move |_| {
        let _stage = project.with(|p| p.as_ref().map(|p| p.stage_entered_at));
        let args = serde_json::json!({ "project_id": id.get_value() });
        spawn_local(async move {
            if let Ok(list) = ipc::call::<_, Vec<StageEvent>>(cmd::LIST_STAGE_EVENTS, &args).await {
                events.set(list);
            }
        });
    });

    // 名下的产出:打开时取;产出计数变了(别处出了图、建了模型、保存了假设)再取
    let overview = RwSignal::new(None::<ProjectOverview>);
    Effect::new(move |_| {
        let _facts = id.with_value(|id| state.facts_of(id));
        let args = serde_json::json!({ "id": id.get_value() });
        spawn_local(async move {
            match ipc::call::<_, ProjectOverview>(cmd::PROJECT_OVERVIEW, &args).await {
                Ok(o) => overview.set(Some(o)),
                Err(e) => state.notify_error(e),
            }
        });
    });
    state.reload_project_facts();
    // 打样与定价的摘要:和产出一样,清单变了就重取
    let pricing = RwSignal::new(PricingSummary::default());
    Effect::new(move |_| {
        let _facts = id.with_value(|id| state.facts_of(id));
        let args = serde_json::json!({ "project_id": id.get_value() });
        spawn_local(async move {
            if let Ok(s) = ipc::call::<_, PricingSummary>(cmd::PRICING_SUMMARY, &args).await {
                pricing.set(s);
            }
        });
    });

    let show_kill = RwSignal::new(false);
    let show_delete = RwSignal::new(false);
    let kill_reason = RwSignal::new(String::new());
    let forced = RwSignal::new(None::<ForcedMove>);
    let tz = local_tz_offset_minutes();

    let status = move || project.with(|p| p.as_ref().map(|p| p.status));
    let stage = move || project.with(|p| p.as_ref().map(|p| p.stage)).unwrap_or(Stage::Idea);
    let facts = move || id.with_value(|id| state.facts_of(id));
    let set_status = move |to: ProjectStatus, reason: Option<String>| {
        ctl.set_status(id.get_value(), to, reason, move || show_kill.set(false));
    };
    // 进入下一阶段:证据够就直接走,不够就先问一句原因(和看板拖拽同一套规则)
    let advance = move || {
        if let Some(next) = stage().next() {
            ctl.request_move(id.get_value(), next, forced);
        }
    };
    let with_project = move |f: &dyn Fn(&Project)| {
        if let Some(p) = project.get_untracked() {
            f(&p);
        }
    };
    let adopted_image = move || overview.with_untracked(|o| o.as_ref().and_then(|o| o.images.iter().find(|i| i.adopted).map(|i| i.version_id.clone())));
    let start_research = move || with_project(&|p| ctl.start_research(p));
    let start_board = move || with_project(&|p| ctl.start_board(p));
    let start_design = move || with_project(&|p| ctl.start_design(p, adopted_image()));
    let start_pricing = move || with_project(&|p| ctl.start_pricing(p));
    let start_publish = move || with_project(&|p| ctl.start_publish(p));

    view! {
        <div class="flex items-center gap-2 flex-wrap">
            {move || project.get().map(|p| {
                let tone = match p.status {
                    ProjectStatus::Active => Tone::Brand,
                    ProjectStatus::Paused => Tone::Amber,
                    ProjectStatus::Killed => Tone::Red,
                    ProjectStatus::Done => Tone::Green,
                };
                // 语言要在这个闭包里读(被跟踪);Badge 的 children 是稍后才执行的,那时已经不在跟踪上下文里
                let status = status_name(i18n.get_locale(), p.status);
                view! { <Badge tone=tone>{status}</Badge> }
            })}
            <span class="text-xs tabular-nums text-gray-400">
                {move || overview.with(|o| o.as_ref().filter(|o| o.cost_fen > 0).map(|o| {
                    t_string!(i18n, project.cost_so_far, yuan = format!("{:.2}", o.cost_fen as f64 / 100.0)).to_string()
                }))}
            </span>
        </div>

        {move || project.get().and_then(|p| p.kill_reason).map(|reason| view! {
            <div class="rounded-lg bg-red-50 dark:bg-red-500/10 border border-red-200 dark:border-red-500/30 px-3 py-2 text-xs text-red-700 dark:text-red-300 selectable">
                <span class="font-medium">{move || t_string!(i18n, project.killed_because)}": "</span>
                {reason}
            </div>
        })}

        // ---- 阶段条 ----
        <ol class="flex items-center gap-1">
            {Stage::ALL.into_iter().map(|s| {
                let current = move || stage() == s;
                let past = move || s.index() < stage().index();
                view! {
                    <li class="flex-1 min-w-0 space-y-1">
                        <div
                            class="h-1.5 rounded-full"
                            class=("bg-brand", current)
                            class=("bg-brand/40", past)
                            class=("bg-gray-200", move || !current() && !past())
                            class=("dark:bg-gray-700", move || !current() && !past())
                        ></div>
                        <div
                            class="text-[11px] text-center truncate"
                            class=("font-semibold", current)
                            class=("text-brand", current)
                            class=("text-gray-400", move || !current())
                        >
                            {move || stage_name(i18n.get_locale(), s)}
                        </div>
                    </li>
                }
            }).collect_view()}
        </ol>

        // ---- 下一步:这个阶段的清单 + 该去哪个工作台 ----
        <section class="rounded-xl border border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-900/40 p-4 space-y-3">
            <div class="flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold text-gray-900 dark:text-gray-50">
                    {move || t_string!(i18n, project.gate_title, stage = stage_name(i18n.get_locale(), stage())).to_string()}
                </h3>
                <Show when=move || gate_passed(stage(), &facts())>
                    <Badge tone=Tone::Green>{move || t_string!(i18n, project.gate_ready)}</Badge>
                </Show>
            </div>
            {move || {
                let items = stage_gate(stage(), &facts());
                if items.is_empty() {
                    return view! {
                        <p class="text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || t_string!(i18n, project.gate_manual)}</p>
                    }.into_any();
                }
                items.into_iter().map(|item| view! {
                    <div class="flex items-center gap-2 text-sm">
                        <span
                            class="w-4 h-4 shrink-0 rounded-full flex items-center justify-center"
                            class=("bg-green-500", item.done)
                            class=("text-white", item.done)
                            class=("border", !item.done)
                            class=("border-gray-300", !item.done)
                            class=("dark:border-gray-600", !item.done)
                        >
                            {item.done.then(|| view! { <Icon kind=IconKind::Check class="w-3 h-3"/> })}
                        </span>
                        <span class=("text-gray-400", item.done) class=("line-through", item.done) class=("text-gray-800", !item.done) class=("dark:text-gray-100", !item.done)>
                            {move || gate_label(i18n.get_locale(), item.key)}
                        </span>
                    </div>
                }).collect_view().into_any()
            }}
            <Show when=move || stage() == Stage::Idea && !facts().has_hypothesis>
                <p class="text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || t_string!(i18n, project.hint_hypothesis)}</p>
            </Show>
            <div class="flex items-center gap-2 flex-wrap pt-1">
                // 这个阶段该去的工作台(清单没完成时是主按钮)
                {move || {
                    let done = gate_passed(stage(), &facts());
                    let variant = if done { ButtonVariant::Secondary } else { ButtonVariant::Primary };
                    match stage() {
                        Stage::Idea | Stage::Research => view! {
                            <Button small=true variant=variant icon=IconKind::Lightbulb on_click=start_research>{move || t_string!(i18n, project.do_research)}</Button>
                        }.into_any(),
                        Stage::Concept => view! {
                            <Button small=true variant=variant icon=IconKind::Image on_click=start_board>{move || t_string!(i18n, project.do_images)}</Button>
                        }.into_any(),
                        Stage::Model => view! {
                            <Button small=true variant=variant icon=IconKind::Box on_click=start_design>
                                {move || if facts().adopted_images > 0 { t_string!(i18n, project.do_model_from_image) } else { t_string!(i18n, project.do_model) }}
                            </Button>
                        }.into_any(),
                        Stage::Prototype => view! {
                            <Button small=true variant=variant icon=IconKind::Ruler on_click=start_pricing>{move || t_string!(i18n, project.do_pricing)}</Button>
                        }.into_any(),
                        Stage::Listing => view! {
                            <Button small=true variant=variant icon=IconKind::Upload on_click=start_publish>{move || t_string!(i18n, project.do_publish)}</Button>
                        }.into_any(),
                        _ => ().into_any(),
                    }
                }}
                <div class="flex-1"></div>
                {move || stage().next().map(|next| {
                    let ready = gate_passed(stage(), &facts()) || stage_gate(stage(), &facts()).is_empty();
                    let variant = if gate_passed(stage(), &facts()) { ButtonVariant::Primary } else { ButtonVariant::Ghost };
                    view! {
                        <Button small=true variant=variant on_click=advance>
                            {move || {
                                let name = stage_name(i18n.get_locale(), next);
                                if ready {
                                    t_string!(i18n, project.advance, stage = name).to_string()
                                } else {
                                    t_string!(i18n, project.advance_anyway, stage = name).to_string()
                                }
                            }}
                        </Button>
                    }
                })}
            </div>
        </section>

        // ---- 名下的产出 ----
        <section class="space-y-2">
            <div class="flex items-center justify-between">
                <SectionTitle title=move || t_string!(i18n, project.work_research)/>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Plus on_click=start_research>{move || t_string!(i18n, project.do_research)}</Button>
            </div>
            {move || overview.with(|o| match o.as_ref().map(|o| o.research.clone()).unwrap_or_default() {
                list if list.is_empty() => view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, project.none_research)}</p> }.into_any(),
                list => list.into_iter().map(|r| {
                    let run_id = r.id.clone();
                    view! {
                        <button
                            type="button"
                            class="w-full flex items-center justify-between gap-3 px-3 py-2 rounded-lg border border-gray-200 dark:border-gray-700 text-left \
                                   hover:border-brand transition-colors"
                            on:click=move |_| ctl.open_research(run_id.clone())
                        >
                            <span class="min-w-0 truncate text-sm text-gray-800 dark:text-gray-100">{r.topic.clone()}</span>
                            <span class="shrink-0 text-[11px] tabular-nums text-gray-400">
                                {move || t_string!(i18n, project.opportunities, n = r.opportunities).to_string()}" · "{format_ts(r.created_at, tz)}
                            </span>
                        </button>
                    }
                }).collect_view().into_any(),
            })}
        </section>

        <section class="space-y-2">
            <div class="flex items-center justify-between">
                <SectionTitle title=move || t_string!(i18n, project.work_images)/>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Plus on_click=start_board>{move || t_string!(i18n, project.do_images)}</Button>
            </div>
            {move || overview.with(|o| {
                let (boards, images) = o.as_ref().map(|o| (o.boards.clone(), o.images.clone())).unwrap_or_default();
                if boards.is_empty() {
                    return view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, project.none_images)}</p> }.into_any();
                }
                view! {
                    // 图:采用的排前面,点一张就打开它所在的画板
                    <div class="grid grid-cols-6 gap-1.5">
                        {images.into_iter().map(|img| {
                            let board_id = img.board_id.clone();
                            view! {
                                <button
                                    type="button"
                                    class="relative aspect-square rounded-md overflow-hidden border border-gray-200 dark:border-gray-700 hover:border-brand transition-colors"
                                    on:click=move |_| ctl.open_board(board_id.clone())
                                >
                                    <img src=ipc::asset_url(&img.asset_id) class="w-full h-full object-cover" draggable="false"/>
                                    {img.adopted.then(|| view! {
                                        <span class="absolute left-0.5 top-0.5 w-3.5 h-3.5 rounded-full bg-green-500 text-white flex items-center justify-center">
                                            <Icon kind=IconKind::Check class="w-2.5 h-2.5"/>
                                        </span>
                                    })}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    <div class="flex flex-wrap gap-1.5">
                        {boards.into_iter().map(|b| {
                            let board_id = b.id.clone();
                            view! {
                                <button
                                    type="button"
                                    class="px-2 py-1 rounded-md border border-gray-200 dark:border-gray-700 text-[11px] text-gray-600 dark:text-gray-300 hover:border-brand transition-colors"
                                    on:click=move |_| ctl.open_board(board_id.clone())
                                >
                                    {b.name.clone()}" · "{move || t_string!(i18n, project.images_count, n = b.images).to_string()}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            })}
        </section>

        <section class="space-y-2">
            <div class="flex items-center justify-between">
                <SectionTitle title=move || t_string!(i18n, project.work_models)/>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Plus on_click=start_design>{move || t_string!(i18n, project.do_model)}</Button>
            </div>
            {move || overview.with(|o| match o.as_ref().map(|o| o.designs.clone()).unwrap_or_default() {
                list if list.is_empty() => view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, project.none_models)}</p> }.into_any(),
                list => list.into_iter().map(|d| {
                    let design_id = d.id.clone();
                    let size = d.size.map(|s| format!("{} × {} × {} mm", fmt_mm(s[0]), fmt_mm(s[1]), fmt_mm(s[2])));
                    view! {
                        <button
                            type="button"
                            class="w-full flex items-center gap-3 px-2 py-2 rounded-lg border border-gray-200 dark:border-gray-700 text-left hover:border-brand transition-colors"
                            on:click=move |_| ctl.open_design(design_id.clone())
                        >
                            <div class="w-11 h-11 shrink-0 rounded-md overflow-hidden bg-gray-100 dark:bg-gray-900 flex items-center justify-center text-gray-300 dark:text-gray-600">
                                {match d.thumb.clone() {
                                    Some(thumb) => view! { <img src=thumb class="w-full h-full object-cover" draggable="false"/> }.into_any(),
                                    None => view! { <Icon kind=IconKind::Box class="w-5 h-5"/> }.into_any(),
                                }}
                            </div>
                            <div class="min-w-0 flex-1">
                                <div class="text-sm truncate text-gray-800 dark:text-gray-100">{d.name.clone()}</div>
                                <div class="text-[11px] tabular-nums text-gray-400">
                                    {size.unwrap_or_else(|| "—".into())}" · "{move || t_string!(i18n, project.versions_count, n = d.versions).to_string()}
                                </div>
                            </div>
                        </button>
                    }
                }).collect_view().into_any(),
            })}
        </section>

        <section class="space-y-2">
            <div class="flex items-center justify-between">
                <SectionTitle title=move || t_string!(i18n, project.work_pricing)/>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Ruler on_click=start_pricing>{move || t_string!(i18n, project.do_pricing)}</Button>
            </div>
            {move || {
                let s = pricing.get();
                if s.unit_cost <= 0.0 && s.runs == 0 {
                    return view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, project.none_pricing)}</p> }.into_any();
                }
                let price = s.chosen_price.map(|p| format!("¥{p:.2}")).unwrap_or_else(|| "—".into());
                view! {
                    <button
                        type="button"
                        class="w-full grid grid-cols-3 gap-2 px-3 py-2 rounded-lg border border-gray-200 dark:border-gray-700 text-center hover:border-brand transition-colors"
                        on:click=move |_| start_pricing()
                    >
                        <div>
                            <div class="text-sm font-semibold tabular-nums text-gray-900 dark:text-gray-50">{format!("¥{:.2}", s.unit_cost)}</div>
                            <div class="text-[11px] text-gray-400">{move || t_string!(i18n, pricing.unit_cost)}</div>
                        </div>
                        <div>
                            <div class="text-sm font-semibold tabular-nums text-gray-900 dark:text-gray-50">{price}</div>
                            <div class="text-[11px] text-gray-400">{move || t_string!(i18n, pricing.your_price)}</div>
                        </div>
                        <div>
                            <div class="text-sm font-semibold tabular-nums text-gray-900 dark:text-gray-50">{format!("{} / {}", s.successes, s.runs)}</div>
                            <div class="text-[11px] text-gray-400">{move || t_string!(i18n, project.runs_ok)}</div>
                        </div>
                    </button>
                }.into_any()
            }}
        </section>

        <section class="space-y-3">
            <SectionTitle title=move || t_string!(i18n, project.details)/>
            <Field label=move || t_string!(i18n, project.title_label)>
                <TextInput value=title/>
            </Field>
            <Field label=move || t_string!(i18n, project.category_label)>
                <TextInput value=category placeholder=move || t_string!(i18n, project.category_placeholder)/>
            </Field>
            <Field
                label=move || t_string!(i18n, project.hypothesis_label)
                hint=move || t_string!(i18n, project.hypothesis_hint)
            >
                <TextArea value=hypothesis rows=4 placeholder=move || t_string!(i18n, project.hypothesis_placeholder)/>
            </Field>
            <div class="flex justify-end">
                <Button
                    small=true
                    disabled=Signal::derive(move || !can_save.get())
                    on_click=move || ctl.update(
                        id.get_value(),
                        title.get_untracked(),
                        category.get_untracked(),
                        hypothesis.get_untracked(),
                    )
                >
                    {move || t_string!(i18n, common.save)}
                </Button>
            </div>
        </section>

        <section class="space-y-3">
            <SectionTitle title=move || t_string!(i18n, project.timeline)/>
            <ol class="space-y-2">
                <For each=move || events.get() key=|e| e.id.clone() let:e>
                    <li class="flex gap-3 text-xs">
                        <span class="w-24 shrink-0 tabular-nums text-gray-400">{format_ts(e.created_at, tz)}</span>
                        <div class="min-w-0 space-y-0.5">
                            <div class="flex items-center gap-1.5 flex-wrap text-gray-700 dark:text-gray-200">
                                {match e.from_stage {
                                    None => view! { <span>{move || t_string!(i18n, project.event_created)}</span> }.into_any(),
                                    Some(from) => view! {
                                        <span>{move || stage_name(i18n.get_locale(), from)}</span>
                                        <span class="text-gray-400">"→"</span>
                                        <span class="font-medium">{move || stage_name(i18n.get_locale(), e.to_stage)}</span>
                                    }.into_any(),
                                }}
                                {e.forced.then(|| view! {
                                    <Badge tone=Tone::Amber>{move || t_string!(i18n, project.event_forced)}</Badge>
                                })}
                            </div>
                            {(!e.note.is_empty()).then(|| view! {
                                <p class="text-gray-500 dark:text-gray-400 break-words selectable">{e.note.clone()}</p>
                            })}
                        </div>
                    </li>
                </For>
            </ol>
        </section>

        <section class="flex items-center gap-2 pt-2 border-t border-gray-200 dark:border-gray-700">
            <Show when=move || status() == Some(ProjectStatus::Active)>
                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Pause
                    on_click=move || set_status(ProjectStatus::Paused, None)>
                    {move || t_string!(i18n, project.pause)}
                </Button>
            </Show>
            <Show when=move || matches!(status(), Some(ProjectStatus::Paused | ProjectStatus::Killed | ProjectStatus::Done))>
                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Play
                    on_click=move || set_status(ProjectStatus::Active, None)>
                    {move || t_string!(i18n, project.resume)}
                </Button>
            </Show>
            <Show when=move || matches!(status(), Some(ProjectStatus::Active | ProjectStatus::Paused))>
                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Ban
                    on_click=move || { kill_reason.set(String::new()); show_kill.set(true); }>
                    {move || t_string!(i18n, project.kill)}
                </Button>
            </Show>
            <div class="flex-1"></div>
            <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Trash on_click=move || show_delete.set(true)>
                {move || t_string!(i18n, common.delete)}
            </Button>
        </section>

        <ForcedMoveDialog pending=forced ctl=ctl/>

        <Dialog open=show_kill title=move || t_string!(i18n, project.kill_title)>
            <Field
                label=move || t_string!(i18n, project.kill_reason_label)
                hint=move || t_string!(i18n, project.kill_reason_hint)
            >
                <TextInput
                    value=kill_reason
                    autofocus=true
                    placeholder=move || t_string!(i18n, project.kill_reason_placeholder)
                />
            </Field>
            <DialogFooter>
                <Button variant=ButtonVariant::Ghost on_click=move || show_kill.set(false)>
                    {move || t_string!(i18n, common.cancel)}
                </Button>
                <Button
                    variant=ButtonVariant::Danger
                    disabled=Signal::derive(move || kill_reason.with(|r| r.trim().is_empty()))
                    on_click=move || set_status(ProjectStatus::Killed, Some(kill_reason.get_untracked()))
                >
                    {move || t_string!(i18n, project.kill)}
                </Button>
            </DialogFooter>
        </Dialog>

        <Dialog open=show_delete title=move || t_string!(i18n, project.delete_title)>
            <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">
                {move || {
                    let code = project.with(|p| p.as_ref().map(|p| p.code.clone()).unwrap_or_default());
                    t_string!(i18n, project.delete_confirm, code = code).to_string()
                }}
            </p>
            <DialogFooter>
                <Button variant=ButtonVariant::Ghost on_click=move || show_delete.set(false)>
                    {move || t_string!(i18n, common.cancel)}
                </Button>
                <Button
                    variant=ButtonVariant::Danger
                    on_click=move || {
                        let code = project.with_untracked(|p| p.as_ref().map(|p| p.code.clone()).unwrap_or_default());
                        show_delete.set(false);
                        ctl.delete(id.get_value(), code);
                    }
                >
                    {move || t_string!(i18n, common.delete)}
                </Button>
            </DialogFooter>
        </Dialog>
    }
}
