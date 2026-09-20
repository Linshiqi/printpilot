//! 项目详情抽屉:基本信息、状态操作、阶段流转时间线。
//! MVP-α 会把它升级成完整的项目详情页(各阶段一个标签页),抽屉保留为看板上的速览。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::t_string;
use pp_common::{Project, ProjectStatus, StageEvent};

use crate::controller::ProjectController;
use crate::i18n::use_i18n;
use crate::i18n_util::{stage_name, status_name};
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::ui::{Badge, Button, ButtonVariant, Dialog, DialogFooter, Drawer, Field, SectionTitle, TextArea, TextInput, Tone};
use crate::utils::{format_ts, local_tz_offset_minutes};

#[component]
pub fn ProjectDrawer(state: AppState) -> impl IntoView {
    let open = RwSignal::new(false);
    Effect::new(move |_| {
        if state.open_project.with(Option::is_some) {
            open.set(true);
        }
    });
    Effect::new(move |_| {
        if !open.get() {
            state.open_project.set(None);
        }
    });

    let title = move || {
        let id = state.open_project.get();
        state
            .projects
            .with(|list| list.iter().find(|p| Some(&p.id) == id.as_ref()).map(|p| p.code.clone()))
            .unwrap_or_default()
    };

    view! {
        <Drawer open=open title=title>
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

    let show_kill = RwSignal::new(false);
    let show_delete = RwSignal::new(false);
    let kill_reason = RwSignal::new(String::new());
    let tz = local_tz_offset_minutes();

    let status = move || project.with(|p| p.as_ref().map(|p| p.status));
    let set_status = move |to: ProjectStatus, reason: Option<String>| {
        ctl.set_status(id.get_value(), to, reason, move || show_kill.set(false));
    };

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
                let locale = i18n.get_locale();
                let (stage, status) = (stage_name(locale, p.stage), status_name(locale, p.status));
                view! {
                    <Badge tone=Tone::Neutral>{stage}</Badge>
                    <Badge tone=tone>{status}</Badge>
                }
            })}
        </div>

        {move || project.get().and_then(|p| p.kill_reason).map(|reason| view! {
            <div class="rounded-lg bg-red-50 dark:bg-red-500/10 border border-red-200 dark:border-red-500/30 px-3 py-2 text-xs text-red-700 dark:text-red-300 selectable">
                <span class="font-medium">{move || t_string!(i18n, project.killed_because)}": "</span>
                {reason}
            </div>
        })}

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
