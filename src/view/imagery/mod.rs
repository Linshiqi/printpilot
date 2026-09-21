//! 图片工作台(一级入口)。接图片生成模型的接口出图,并且围绕**一条和 AI 的持续对话**来改图。
//!
//!   左:画板列表(一个画板 = 一个主题的一组图:参考图 + 对话 + 生成过的每一张)
//!   中:当前选中的图(大图)+ 这条线上所有图的胶片条;采用 / 送去建模 / 导出
//!   右:对话——第一句话出图;之后每句话要么改当前这张、要么重新出一批、要么只是回答
//!
//! 和建模工作室同构,思路见 docs/adr/0005-image-studio.md。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::design::CadDesign;
use pp_common::imagery::{
    ImageAspect, ImageBoard, ImageBoardDetail, ImageBoardSummary, ImageMessage, ImageMsgKind, ImageProviderInfo, ImagePurpose, ImageTurnResult,
    ImageVersion,
};
use pp_common::Asset;

use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::current_locale;
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::{AppState, Handoff, Route};
use crate::theme::{get_pref, set_pref};
use crate::ui::{Badge, Button, ButtonVariant, Dialog, DialogFooter, EmptyState, IconButton, Segmented, Tone};

/// 一轮最多涉及 3 张图(当前选中的那张也算一张)——和后端的 MAX_TURN_IMAGES 一致
const MAX_TURN_IMAGES: usize = 3;

fn purpose_name(l: Locale, p: ImagePurpose) -> &'static str {
    match p {
        ImagePurpose::ModelRef => td_string!(l, imagery.purpose_model_ref),
        ImagePurpose::Scene => td_string!(l, imagery.purpose_scene),
        ImagePurpose::Cover => td_string!(l, imagery.purpose_cover),
        ImagePurpose::Free => td_string!(l, imagery.purpose_free),
    }
}

#[component]
pub fn ImageryView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();

    // ---- 状态 ----
    let info = RwSignal::new(None::<ImageProviderInfo>);
    let boards = RwSignal::new(Vec::<ImageBoardSummary>::new());
    let board = RwSignal::new(None::<ImageBoard>);
    let messages = RwSignal::new(Vec::<ImageMessage>::new());
    let versions = RwSignal::new(Vec::<ImageVersion>::new());
    let busy = RwSignal::new(false);
    let text = RwSignal::new(String::new());
    let pending = RwSignal::new(Vec::<Asset>::new());
    let count = RwSignal::new(get_pref("imagery_count").and_then(|c| c.parse::<u32>().ok()).unwrap_or(2));
    let delete_dialog = RwSignal::new(false);
    let remove_dialog = RwSignal::new(false);
    let renaming = RwSignal::new(None::<String>);

    let demo = move || state.app_info.with(|i| i.as_ref().is_some_and(|i| i.demo_mode));
    let board_id = move || board.with_untracked(|b| b.as_ref().map(|b| b.id.clone()));
    let current = Memo::new(move |_| {
        let id = board.with(|b| b.as_ref().and_then(|b| b.current_image_id.clone()))?;
        versions.with(|list| list.iter().find(|v| v.id == id).cloned())
    });
    // 能发消息 = 出图的供应商就绪 + 规划用的 DeepSeek 也就绪(每一轮都要它先看图、写提示词)
    let ready = move || info.with(|i| i.as_ref().is_some_and(|i| i.available && i.planner_ready));

    // ---- 数据 ----
    let load_info = move || {
        spawn_local(async move {
            match ipc::call_no_args::<ImageProviderInfo>(cmd::IMAGE_PROVIDER_INFO).await {
                Ok(i) => info.set(Some(i)),
                Err(e) => {
                    state.notify_error(e);
                    info.set(Some(ImageProviderInfo::default()));
                }
            }
        });
    };
    load_info();
    let reload_list = move || {
        spawn_local(async move {
            match ipc::call_no_args::<Vec<ImageBoardSummary>>(cmd::BOARD_LIST).await {
                Ok(list) => boards.set(list),
                Err(e) => state.notify_error(e),
            }
        });
    };
    // `draft`:从项目里过来时,输入框里先放一句由项目信息拼出来的话(用户可以改)
    let open_with = move |id: String, draft: String| {
        spawn_local(async move {
            match ipc::call::<_, ImageBoardDetail>(cmd::BOARD_GET, &serde_json::json!({ "id": id })).await {
                Ok(d) => {
                    set_pref("imagery_board", &d.board.id);
                    text.set(draft);
                    pending.set(Vec::new());
                    messages.set(d.messages);
                    versions.set(d.versions);
                    board.set(Some(d.board));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let open = move |id: String| open_with(id, String::new());
    let new_board = move |purpose: ImagePurpose| {
        spawn_local(async move {
            match ipc::call::<_, ImageBoard>(cmd::BOARD_CREATE, &serde_json::json!({ "purpose": purpose, "project_id": null, "name": null })).await {
                Ok(b) => {
                    reload_list();
                    open(b.id);
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    // 从项目里过来的:打开指定的画板,并带上那句草稿;否则回到上次打开的那个
    let (wanted, draft) = match state.handoff.get_untracked() {
        Some(Handoff::Board { board_id, draft }) => {
            state.handoff.set(None);
            (Some(board_id), draft)
        }
        _ => (get_pref("imagery_board"), String::new()),
    };
    spawn_local(async move {
        match ipc::call_no_args::<Vec<ImageBoardSummary>>(cmd::BOARD_LIST).await {
            Ok(list) => {
                let pick = list.iter().find(|b| Some(&b.id) == wanted.as_ref()).or(list.first()).map(|b| b.id.clone());
                // 草稿只属于指定的那个画板:它要是不在了,别把话塞进别的画板
                let draft = if pick == wanted { draft } else { String::new() };
                boards.set(list);
                if let Some(id) = pick {
                    open_with(id, draft);
                }
            }
            Err(e) => state.notify_error(e),
        }
    });

    let send_text = move |said: String| {
        let Some(id) = board_id() else { return };
        let images: Vec<String> = pending.with_untracked(|l| l.iter().map(|a| a.id.clone()).collect());
        if busy.get_untracked() || (said.trim().is_empty() && images.is_empty()) {
            return;
        }
        let args = serde_json::json!({ "id": id, "text": said, "image_asset_ids": images, "count": count.get_untracked() });
        state.image_progress.set(None);
        busy.set(true);
        spawn_local(async move {
            // 失败时什么都没入库:输入框里的话留着,改好配置再发一次
            match ipc::call::<_, ImageTurnResult>(cmd::BOARD_SEND, &args).await {
                Ok(res) => {
                    text.set(String::new());
                    pending.set(Vec::new());
                    versions.update(|l| {
                        for v in res.versions.into_iter().rev() {
                            l.insert(0, v);
                        }
                    });
                    messages.update(|l| l.extend(res.messages));
                    board.set(Some(res.board));
                    reload_list();
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
            busy.set(false);
            state.image_progress.set(None);
        });
    };
    let send = move || send_text(text.get_untracked());
    // 还能再贴几张:当前选中的那张也占一个名额
    let attach_room = move || MAX_TURN_IMAGES.saturating_sub(pending.with(Vec::len) + current.with(|c| c.is_some() as usize));
    let attach = move || {
        let Some(id) = board_id() else { return };
        if attach_room() == 0 {
            return;
        }
        spawn_local(async move {
            let Some(path) = ipc::pick_file("Image", &["png", "jpg", "jpeg", "webp"]).await else {
                return;
            };
            match ipc::call::<_, Asset>(cmd::BOARD_ADD_IMAGE, &serde_json::json!({ "id": id, "path": path })).await {
                Ok(asset) => pending.update(|l| l.push(asset)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let select = move |version_id: String| {
        let Some(id) = board_id() else { return };
        spawn_local(async move {
            match ipc::call::<_, ImageBoard>(cmd::BOARD_SELECT, &serde_json::json!({ "id": id, "version_id": version_id })).await {
                Ok(b) => {
                    board.set(Some(b));
                    reload_list();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let toggle_adopt = move || {
        let Some(v) = current.get_untracked() else { return };
        spawn_local(async move {
            let args = serde_json::json!({ "version_id": v.id, "adopted": !v.adopted });
            match ipc::call::<_, ImageVersion>(cmd::BOARD_ADOPT, &args).await {
                Ok(updated) => {
                    versions.update(|l| {
                        if let Some(slot) = l.iter_mut().find(|x| x.id == updated.id) {
                            *slot = updated;
                        }
                    });
                    // 「采用了一张预览图」是项目「概念」阶段的清单项
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let remove_image = move || {
        remove_dialog.set(false);
        let Some(v) = current.get_untracked() else { return };
        spawn_local(async move {
            match ipc::call::<_, ImageBoard>(cmd::BOARD_REMOVE_IMAGE, &serde_json::json!({ "version_id": v.id })).await {
                Ok(b) => {
                    versions.update(|l| l.retain(|x| x.id != v.id));
                    board.set(Some(b));
                    reload_list();
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    // 送去建模:用这张图新建一个设计,跳到「建模」打开它
    let to_design = move || {
        let Some(v) = current.get_untracked() else { return };
        spawn_local(async move {
            match ipc::call::<_, CadDesign>(cmd::BOARD_TO_DESIGN, &serde_json::json!({ "version_id": v.id })).await {
                Ok(d) => {
                    set_pref("studio_design", &d.id);
                    state.go(Route::Studio);
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let export = move || {
        let Some(v) = current.get_untracked() else { return };
        let name = board.with_untracked(|b| b.as_ref().map(|b| b.name.clone())).unwrap_or_else(|| "image".into());
        spawn_local(async move {
            let Some(dest) = ipc::pick_save_path("Export", &format!("{name}.jpg"), "jpg").await else {
                return;
            };
            match ipc::call_unit(cmd::BOARD_EXPORT, &serde_json::json!({ "version_id": v.id, "dest_path": dest })).await {
                Ok(()) => state.notify_info(td_string!(current_locale(), lab.exported)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let set_options = move |purpose: ImagePurpose, aspect: ImageAspect| {
        let Some(id) = board_id() else { return };
        spawn_local(async move {
            let args = serde_json::json!({ "id": id, "purpose": purpose, "aspect": aspect });
            match ipc::call::<_, ImageBoard>(cmd::BOARD_SET_OPTIONS, &args).await {
                Ok(b) => board.set(Some(b)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let link_project = move |project_id: Option<String>| {
        let Some(id) = board_id() else { return };
        spawn_local(async move {
            match ipc::call::<_, ImageBoard>(cmd::BOARD_LINK_PROJECT, &serde_json::json!({ "id": id, "project_id": project_id })).await {
                Ok(b) => {
                    board.set(Some(b));
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let rename = move || {
        let (Some(id), Some(name)) = (board_id(), renaming.get_untracked()) else { return };
        renaming.set(None);
        if name.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            match ipc::call::<_, ImageBoard>(cmd::BOARD_RENAME, &serde_json::json!({ "id": id, "name": name })).await {
                Ok(b) => {
                    board.set(Some(b));
                    reload_list();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let delete_board = move || {
        let Some(id) = board_id() else { return };
        delete_dialog.set(false);
        spawn_local(async move {
            match ipc::call_unit(cmd::BOARD_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    board.set(None);
                    messages.set(Vec::new());
                    versions.set(Vec::new());
                    boards.update(|l| l.retain(|b| b.id != id));
                    if let Some(next) = boards.with_untracked(|l| l.first().map(|b| b.id.clone())) {
                        open(next);
                    }
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    Effect::new(move |_| set_pref("imagery_count", &count.get().to_string()));

    // 有新消息 / 开始干活时滚到底
    let scroller = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let _ = (messages.with(Vec::len), busy.get());
        request_animation_frame(move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    });

    let can_send = Signal::derive(move || {
        !busy.get() && ready() && board.with(Option::is_some) && (!text.with(|t| t.trim().is_empty()) || !pending.with(Vec::is_empty))
    });
    let no_image = Signal::derive(move || busy.get() || current.with(Option::is_none));
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    let purpose_of = move || board.with(|b| b.as_ref().map(|b| b.purpose).unwrap_or_default());
    let aspect_of = move || board.with(|b| b.as_ref().map(|b| b.aspect).unwrap_or_default());
    let count_options: Vec<(u32, Signal<String>)> = [1u32, 2, 4].into_iter().map(|n| (n, Signal::stored(format!("×{n}")))).collect();
    let aspect_options: Vec<(ImageAspect, Signal<String>)> = ImageAspect::ALL.into_iter().map(|a| (a, Signal::stored(a.ratio().to_string()))).collect();

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center justify-between gap-4 border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, imagery.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, imagery.subtitle)}</p>
                </div>
                // 现在用的是哪家、能不能改图——「改一下」的效果取决于它
                <div class="shrink-0 text-xs text-right">
                    {move || match info.get() {
                        None => view! { <span class="text-gray-400">{move || t_string!(i18n, common.loading)}</span> }.into_any(),
                        Some(i) if i.available && !i.planner_ready => view! {
                            <button type="button" class="text-amber-600 dark:text-amber-400 hover:underline" on:click=move |_| state.go(Route::Settings)>
                                {move || t_string!(i18n, imagery.no_planner)}
                            </button>
                        }.into_any(),
                        Some(i) if i.available => view! {
                            <div class="flex items-center gap-2">
                                <span class="w-1.5 h-1.5 rounded-full bg-green-500"></span>
                                <span class="text-gray-500 dark:text-gray-400 selectable">{format!("{} · {}", i.name, i.model)}</span>
                                {if i.can_edit {
                                    view! { <Badge tone=Tone::Green>{move || t_string!(i18n, imagery.can_edit)}</Badge> }.into_any()
                                } else {
                                    view! { <Badge tone=Tone::Amber>{move || t_string!(i18n, imagery.generate_only)}</Badge> }.into_any()
                                }}
                            </div>
                        }.into_any(),
                        Some(_) => view! {
                            <button type="button" class="text-amber-600 dark:text-amber-400 hover:underline" on:click=move |_| state.go(Route::Settings)>
                                {move || t_string!(i18n, imagery.no_provider)}
                            </button>
                        }.into_any(),
                    }}
                </div>
            </header>

            <div class="flex-1 min-h-0 flex">
                // ---- 左:画板列表 ----
                <aside class="w-52 shrink-0 h-full flex flex-col border-r border-gray-200 dark:border-gray-700">
                    <div class="p-3 space-y-2 border-b border-gray-200 dark:border-gray-700">
                        <Button icon=IconKind::Plus on_click=move || new_board(ImagePurpose::ModelRef)>{move || t_string!(i18n, imagery.new_board)}</Button>
                        <Show when=demo>
                            <p class="text-[11px] leading-relaxed text-amber-600 dark:text-amber-400">{move || t_string!(i18n, imagery.demo_note)}</p>
                        </Show>
                    </div>
                    <div class="flex-1 min-h-0 overflow-y-auto p-2 space-y-1">
                        <Show when=move || boards.with(Vec::is_empty)>
                            <p class="p-3 text-xs leading-relaxed text-gray-400">{move || t_string!(i18n, imagery.no_boards)}</p>
                        </Show>
                        <For each=move || boards.get() key=|b| (b.id.clone(), b.updated_at, b.cover_asset_id.clone(), b.name.clone()) let:row>
                            <BoardRow row=row board=board on_open=open/>
                        </For>
                    </div>
                </aside>

                <div class="relative flex-1 min-w-0 h-full flex">
                    <Show when=move || board.with(Option::is_none)>
                        <div class="absolute inset-0 z-10 bg-gray-50 dark:bg-gray-900">
                            <EmptyState icon=IconKind::Image title=move || t_string!(i18n, imagery.empty_title) hint=move || t_string!(i18n, imagery.empty_hint)>
                                <Button icon=IconKind::Plus on_click=move || new_board(ImagePurpose::ModelRef)>{move || t_string!(i18n, imagery.new_board)}</Button>
                            </EmptyState>
                        </div>
                    </Show>

                    // ---- 中:当前的图 + 胶片条 ----
                    <section class="flex-1 min-w-0 h-full flex flex-col">
                        <div class="shrink-0 h-11 px-3 flex items-center gap-3 border-b border-gray-200 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap overflow-hidden">
                            {move || match renaming.get() {
                                Some(_) => view! {
                                    <input
                                        type="text"
                                        class="w-44 h-7 px-2 rounded-md border border-brand bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100 focus:outline-none"
                                        prop:value=move || renaming.get().unwrap_or_default()
                                        on:input=move |e| renaming.set(Some(event_target_value(&e)))
                                        on:blur=move |_| rename()
                                        on:keydown=move |e| {
                                            if e.key() == "Enter" && !e.is_composing() {
                                                rename();
                                            } else if e.key() == "Escape" {
                                                renaming.set(None);
                                            }
                                        }
                                    />
                                }.into_any(),
                                None => view! {
                                    <button
                                        type="button"
                                        class="min-w-0 max-w-[18rem] truncate text-sm font-semibold text-gray-900 dark:text-gray-50 hover:text-brand"
                                        title=move || t_string!(i18n, studio.rename)
                                        on:click=move |_| renaming.set(board.with_untracked(|b| b.as_ref().map(|b| b.name.clone())))
                                    >
                                        {move || board.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default())}
                                    </button>
                                }.into_any(),
                            }}
                            // 关联到项目:之后出的图会归到这个项目的文件里
                            <select
                                class="h-7 min-w-0 w-32 shrink px-1.5 rounded-md border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-900 text-xs text-gray-600 dark:text-gray-300"
                                title=move || t_string!(i18n, studio.link_project)
                                on:change=move |e| {
                                    let v = event_target_value(&e);
                                    link_project((!v.is_empty()).then_some(v));
                                }
                            >
                                <option value="" selected=move || board.with(|b| b.as_ref().is_some_and(|b| b.project_id.is_none()))>
                                    {move || t_string!(i18n, studio.no_project)}
                                </option>
                                {move || state.projects.get().into_iter().map(|p| {
                                    let pid = p.id.clone();
                                    view! {
                                        <option
                                            value=p.id.clone()
                                            selected=move || board.with(|b| b.as_ref().and_then(|b| b.project_id.as_deref()) == Some(pid.as_str()))
                                        >{format!("{} {}", p.code, p.title)}</option>
                                    }
                                }).collect_view()}
                            </select>
                            // 已经挂在项目上:一键打开项目中枢(抽屉盖在当前页面上,不用离开工作台)
                            <Show when=move || board.with(|x| x.as_ref().is_some_and(|x| x.project_id.is_some()))>
                                <IconButton
                                    icon=IconKind::Kanban
                                    label=move || t_string!(i18n, project.open_hub)
                                    on_click=move || state.open_project.set(board.with_untracked(|x| x.as_ref().and_then(|x| x.project_id.clone())))
                                />
                            </Show>
                            <div class="flex-1"></div>
                            <IconButton icon=IconKind::Trash label=move || t_string!(i18n, imagery.delete_board) on_click=move || delete_dialog.set(true)/>
                        </div>
                        // 第二行:下一次出图用的设置(用途决定提示词怎么写,画幅决定出图比例)。窄窗口下可以横向滚动
                        <div class="shrink-0 h-10 px-3 flex items-center gap-2 border-b border-gray-200 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap overflow-x-auto">
                            <span class="shrink-0">{move || t_string!(i18n, imagery.purpose)}</span>
                            <Segmented
                                value=Signal::derive(purpose_of)
                                options=vec![
                                    (ImagePurpose::ModelRef, label(|l| td_string!(l, imagery.purpose_model_ref))),
                                    (ImagePurpose::Scene, label(|l| td_string!(l, imagery.purpose_scene))),
                                    (ImagePurpose::Cover, label(|l| td_string!(l, imagery.purpose_cover))),
                                    (ImagePurpose::Free, label(|l| td_string!(l, imagery.purpose_free))),
                                ]
                                on_change=move |p: ImagePurpose| set_options(p, p.default_aspect())
                            />
                            <span class="shrink-0 pl-2">{move || t_string!(i18n, imagery.aspect)}</span>
                            <Segmented value=Signal::derive(aspect_of) options=aspect_options on_change=move |a: ImageAspect| set_options(purpose_of(), a)/>
                        </div>
                        // 场景图 / 封面是要发出去的:把平台规则说在前面(docs/04-integrations.md §4.3)
                        <Show when=move || matches!(purpose_of(), ImagePurpose::Scene | ImagePurpose::Cover)>
                            <p class="shrink-0 px-3 py-1.5 text-[11px] leading-relaxed text-amber-700 dark:text-amber-300 bg-amber-50 dark:bg-amber-500/10 border-b border-amber-200 dark:border-amber-500/30">
                                {move || t_string!(i18n, imagery.compliance_note)}
                            </p>
                        </Show>
                        <div class="relative flex-1 min-h-0 bg-gray-100 dark:bg-gray-900 flex items-center justify-center p-4">
                            {move || match current.get() {
                                Some(v) => view! {
                                    <img src=ipc::asset_url(&v.asset_id) class="max-w-full max-h-full object-contain rounded-lg shadow-lg" draggable="false"/>
                                }.into_any(),
                                None => view! {
                                    <div class="pointer-events-none">
                                        <EmptyState icon=IconKind::Image title=move || t_string!(i18n, imagery.viewer_empty) hint=move || t_string!(i18n, imagery.viewer_empty_hint)/>
                                    </div>
                                }.into_any(),
                            }}
                            <Show when=move || current.with(|v| v.as_ref().is_some_and(|v| v.adopted))>
                                <div class="absolute left-3 top-3"><Badge tone=Tone::Green>{move || t_string!(i18n, imagery.adopted)}</Badge></div>
                            </Show>
                            <div class="absolute right-3 bottom-3 flex items-center gap-1.5">
                                <div class="rounded-lg bg-white/90 dark:bg-gray-800/90">
                                    <IconButton
                                        icon=IconKind::Trash
                                        label=move || t_string!(i18n, imagery.remove_image)
                                        disabled=no_image
                                        on_click=move || remove_dialog.set(true)
                                    />
                                </div>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Check disabled=no_image on_click=toggle_adopt>
                                    {move || if current.with(|v| v.as_ref().is_some_and(|v| v.adopted)) {
                                        t_string!(i18n, imagery.unadopt)
                                    } else {
                                        t_string!(i18n, imagery.adopt)
                                    }}
                                </Button>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_image on_click=export>
                                    {move || t_string!(i18n, imagery.export)}
                                </Button>
                                <Button small=true icon=IconKind::Box disabled=no_image on_click=to_design>{move || t_string!(i18n, imagery.to_design)}</Button>
                            </div>
                        </div>
                        // 胶片条:这条线上出过的每一张,最新的在左
                        <Show when=move || !versions.with(Vec::is_empty)>
                            <div class="shrink-0 h-24 px-3 flex items-center gap-2 overflow-x-auto border-t border-gray-200 dark:border-gray-700">
                                <For each=move || versions.get() key=|v| (v.id.clone(), v.adopted) let:v>
                                    <FilmCell version=v current=current on_select=select/>
                                </For>
                            </div>
                        </Show>
                    </section>

                    // ---- 右:对话 ----
                    <aside class="w-[24rem] shrink-0 h-full flex flex-col border-l border-gray-200 dark:border-gray-700">
                        <div node_ref=scroller class="flex-1 min-h-0 overflow-y-auto p-3 space-y-3">
                            <Show when=move || messages.with(Vec::is_empty) && !busy.get()>
                                <div class="pt-8 px-3 text-center space-y-2">
                                    <div class="mx-auto w-10 h-10 rounded-xl bg-brand-soft dark:bg-indigo-500/20 flex items-center justify-center text-brand">
                                        <Icon kind=IconKind::Sparkles class="w-5 h-5"/>
                                    </div>
                                    <p class="text-sm font-medium text-gray-800 dark:text-gray-100">{move || t_string!(i18n, imagery.chat_empty_title)}</p>
                                    <p class="text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || t_string!(i18n, imagery.chat_empty_hint)}</p>
                                </div>
                            </Show>
                            <For each=move || messages.get() key=|m| m.id.clone() let:m>
                                <MessageView msg=m versions=versions current=current on_select=select/>
                            </For>
                            <Show when=move || busy.get()>
                                <div class="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
                                    <span class="inline-flex gap-1">
                                        <span class="w-1.5 h-1.5 rounded-full bg-brand animate-bounce"></span>
                                        <span class="w-1.5 h-1.5 rounded-full bg-brand animate-bounce [animation-delay:120ms]"></span>
                                        <span class="w-1.5 h-1.5 rounded-full bg-brand animate-bounce [animation-delay:240ms]"></span>
                                    </span>
                                    // 这一轮走到哪了:想怎么画 → 图片模型在出图(最慢的一步)→ 保存
                                    {move || match state.image_progress.get() {
                                        Some((phase, provider, n)) if phase == "rendering" => {
                                            t_string!(i18n, imagery.phase_rendering, provider = provider, count = n).to_string()
                                        }
                                        Some((phase, _, _)) if phase == "saving" => t_string!(i18n, imagery.phase_saving).to_string(),
                                        _ => t_string!(i18n, imagery.phase_planning).to_string(),
                                    }}
                                    <div class="flex-1"></div>
                                    // 停止这一轮:什么都不入库,输入框里的话还在。出图请求已经发出去的话,那几张图供应商照样计费
                                    <Button
                                        small=true
                                        variant=ButtonVariant::Secondary
                                        icon=IconKind::Ban
                                        on_click=move || {
                                            if let Some(id) = board_id() {
                                                state.cancel_turn(format!("board:{id}"));
                                            }
                                        }
                                    >
                                        {move || t_string!(i18n, studio.stop)}
                                    </Button>
                                </div>
                            </Show>
                        </div>
                        <div class="shrink-0 border-t border-gray-200 dark:border-gray-700 p-3 space-y-2">
                            <Show when=move || !pending.with(Vec::is_empty)>
                                <div class="flex flex-wrap gap-2">
                                    <For each=move || pending.get() key=|a| a.id.clone() let:asset>
                                        {
                                            let id = StoredValue::new(asset.id.clone());
                                            view! {
                                                <div class="group relative w-12 h-12 rounded-lg overflow-hidden border border-gray-200 dark:border-gray-700">
                                                    <img src=ipc::asset_url(&asset.id) class="w-full h-full object-cover" draggable="false"/>
                                                    <div class="absolute top-0 right-0 opacity-0 group-hover:opacity-100 transition-opacity rounded bg-white/90 dark:bg-gray-800/90">
                                                        <IconButton
                                                            icon=IconKind::Close
                                                            label=move || t_string!(i18n, common.delete)
                                                            on_click=move || pending.update(|l| l.retain(|a| id.with_value(|id| &a.id != id)))
                                                        />
                                                    </div>
                                                </div>
                                            }
                                        }
                                    </For>
                                </div>
                            </Show>
                            <textarea
                                rows=3
                                class="w-full px-3 py-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm leading-relaxed \
                                       text-gray-900 dark:text-gray-100 placeholder:text-gray-400 resize-none focus:outline-none focus:ring-2 focus:ring-brand/40 focus:border-brand"
                                placeholder=move || if current.with(Option::is_some) {
                                    t_string!(i18n, imagery.composer_edit_placeholder)
                                } else {
                                    t_string!(i18n, imagery.composer_new_placeholder)
                                }
                                prop:value=move || text.get()
                                on:input=move |e| text.set(event_target_value(&e))
                                on:keydown=move |e| {
                                    if e.key() == "Enter" && !e.shift_key() && !e.is_composing() {
                                        e.prevent_default();
                                        if can_send.get_untracked() {
                                            send();
                                        }
                                    }
                                }
                            ></textarea>
                            <div class="flex items-center justify-between gap-2 whitespace-nowrap">
                                <div class="flex items-center gap-2">
                                    <IconButton
                                        icon=IconKind::Image
                                        label=move || t_string!(i18n, imagery.attach_image)
                                        disabled=Signal::derive(move || busy.get() || attach_room() == 0)
                                        on_click=attach
                                    />
                                    <Segmented value=Signal::derive(move || count.get()) options=count_options on_change=move |n: u32| count.set(n)/>
                                </div>
                                <Button small=true icon=IconKind::Play disabled=Signal::derive(move || !can_send.get()) on_click=send>
                                    {move || t_string!(i18n, studio.send)}
                                </Button>
                            </div>
                        </div>
                    </aside>
                </div>
            </div>

            <Dialog open=delete_dialog title=move || t_string!(i18n, imagery.delete_board)>
                <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">{move || t_string!(i18n, imagery.delete_confirm)}</p>
                <DialogFooter>
                    <Button variant=ButtonVariant::Secondary on_click=move || delete_dialog.set(false)>{move || t_string!(i18n, common.cancel)}</Button>
                    <Button variant=ButtonVariant::Danger icon=IconKind::Trash on_click=delete_board>{move || t_string!(i18n, common.delete)}</Button>
                </DialogFooter>
            </Dialog>
            <Dialog open=remove_dialog title=move || t_string!(i18n, imagery.remove_image)>
                <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">{move || t_string!(i18n, imagery.remove_image_confirm)}</p>
                <DialogFooter>
                    <Button variant=ButtonVariant::Secondary on_click=move || remove_dialog.set(false)>{move || t_string!(i18n, common.cancel)}</Button>
                    <Button variant=ButtonVariant::Danger icon=IconKind::Trash on_click=remove_image>{move || t_string!(i18n, imagery.remove)}</Button>
                </DialogFooter>
            </Dialog>
        </div>
    }
}

#[component]
fn BoardRow(row: ImageBoardSummary, board: RwSignal<Option<ImageBoard>>, #[prop(into)] on_open: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(row.id.clone());
    let active = move || board.with(|b| b.as_ref().is_some_and(|b| id.with_value(|id| &b.id == id)));
    let purpose = row.purpose;
    let images = row.images;
    view! {
        <div
            class="flex items-center gap-2.5 px-2 py-2 rounded-lg cursor-pointer transition-colors"
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| on_open.run((id.get_value(),))
        >
            <div class="w-11 h-11 shrink-0 rounded-md overflow-hidden bg-gray-100 dark:bg-gray-900 flex items-center justify-center text-gray-300 dark:text-gray-600">
                {match row.cover_asset_id.clone() {
                    Some(asset) => view! { <img src=ipc::asset_url(&asset) class="w-full h-full object-cover" draggable="false"/> }.into_any(),
                    None => view! { <Icon kind=IconKind::Image class="w-5 h-5"/> }.into_any(),
                }}
            </div>
            <div class="min-w-0 flex-1">
                <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{row.name.clone()}</div>
                <div class="text-[11px] text-gray-400 truncate">
                    {move || format!("{} · {}", purpose_name(i18n.get_locale(), purpose), images)}
                </div>
            </div>
        </div>
    }
}

#[component]
fn FilmCell(version: ImageVersion, current: Memo<Option<ImageVersion>>, #[prop(into)] on_select: Callback<(String,)>) -> impl IntoView {
    let id = StoredValue::new(version.id.clone());
    let active = move || current.with(|c| c.as_ref().is_some_and(|c| id.with_value(|id| &c.id == id)));
    view! {
        <button
            type="button"
            class="relative shrink-0 w-[4.5rem] h-[4.5rem] rounded-lg overflow-hidden border-2 transition-colors"
            class=("border-brand", active)
            class=("border-transparent", move || !active())
            title=version.prompt.clone()
            on:click=move |_| on_select.run((id.get_value(),))
        >
            <img src=ipc::asset_url(&version.asset_id) class="w-full h-full object-cover" draggable="false"/>
            {version.adopted.then(|| view! {
                <span class="absolute left-0.5 top-0.5 w-4 h-4 rounded-full bg-green-500 text-white flex items-center justify-center">
                    <Icon kind=IconKind::Check class="w-3 h-3"/>
                </span>
            })}
            {(version.mode == "edit").then(|| view! {
                <span class="absolute right-0.5 bottom-0.5 w-4 h-4 rounded-full bg-amber-500 text-white flex items-center justify-center">
                    <Icon kind=IconKind::Wand class="w-2.5 h-2.5"/>
                </span>
            })}
        </button>
    }
}

#[component]
fn MessageView(
    msg: ImageMessage,
    versions: RwSignal<Vec<ImageVersion>>,
    current: Memo<Option<ImageVersion>>,
    #[prop(into)] on_select: Callback<(String,)>,
) -> impl IntoView {
    let i18n = use_i18n();
    if msg.from_user {
        return view! {
            <div class="flex flex-col items-end gap-1">
                {(!msg.image_asset_ids.is_empty()).then(|| view! {
                    <div class="flex flex-wrap justify-end gap-1.5">
                        {msg.image_asset_ids.iter().map(|id| view! {
                            <img src=ipc::asset_url(id) class="w-16 h-16 object-cover rounded-lg border border-gray-200 dark:border-gray-700" draggable="false"/>
                        }).collect_view()}
                    </div>
                })}
                {(!msg.content.is_empty()).then(|| view! {
                    <div class="max-w-[92%] px-3 py-2 rounded-2xl rounded-br-md bg-brand text-white text-sm leading-relaxed whitespace-pre-wrap selectable">
                        {msg.content.clone()}
                    </div>
                })}
            </div>
        }
        .into_any();
    }

    let report = msg.extra.report.clone();
    let usage = report.map(|r| {
        view! {
            <div class="text-[10px] tabular-nums text-gray-400 selectable">
                {move || t_string!(
                    i18n,
                    imagery.msg_usage,
                    provider = if r.provider.is_empty() { "—".to_string() } else { format!("{} {}", r.provider, r.model) },
                    yuan = format!("{:.3}", r.cost_fen / 100.0),
                    secs = format!("{:.1}", r.elapsed_ms as f64 / 1000.0),
                ).to_string()}
            </div>
        }
    });
    let ids = msg.extra.version_ids.clone();
    let batch = StoredValue::new(ids.clone());
    let all_removed = move || versions.with(|l| batch.with_value(|ids| !ids.iter().any(|id| l.iter().any(|v| &v.id == id))));
    let is_edit = msg.extra.mode.as_deref() == Some("edit");
    let prompt = msg.extra.prompt.clone().unwrap_or_default();

    view! {
        <div class="space-y-1.5">
            {(!msg.content.trim().is_empty()).then(|| view! {
                <div class="max-w-[95%] px-3 py-2 rounded-2xl rounded-bl-md bg-gray-100 dark:bg-gray-700/70 text-sm leading-relaxed whitespace-pre-wrap \
                            text-gray-900 dark:text-gray-100 selectable">
                    {msg.content.clone()}
                </div>
            })}
            {(msg.kind == ImageMsgKind::Images).then(|| view! {
                // 从画板上拿掉的图不再显示;一批全拿掉了就留一句说明,对话本身不删
                <Show when=move || all_removed()>
                    <p class="pl-1 text-[11px] text-gray-400">{move || t_string!(i18n, imagery.images_removed)}</p>
                </Show>
                <div class=if ids.len() == 1 { "grid grid-cols-1 max-w-[70%] gap-1.5" } else { "grid grid-cols-2 gap-1.5" }>
                    {ids.iter().map(|vid| {
                        let vid = StoredValue::new(vid.clone());
                        let asset = move || versions.with(|l| vid.with_value(|id| l.iter().find(|v| &v.id == id).map(|v| v.asset_id.clone())));
                        let active = move || current.with(|c| c.as_ref().is_some_and(|c| vid.with_value(|id| &c.id == id)));
                        view! {
                            <Show when=move || asset().is_some()>
                                <button
                                    type="button"
                                    class="relative rounded-lg overflow-hidden border-2 transition-colors bg-gray-100 dark:bg-gray-900"
                                    class=("border-brand", active)
                                    class=("border-transparent", move || !active())
                                    on:click=move |_| on_select.run((vid.get_value(),))
                                >
                                    {move || asset().map(|a| view! { <img src=ipc::asset_url(&a) class="w-full h-auto block" draggable="false"/> })}
                                </button>
                            </Show>
                        }
                    }).collect_view()}
                </div>
                <details>
                    <summary class="cursor-pointer text-[11px] text-gray-500 dark:text-gray-400">
                        {move || if is_edit { t_string!(i18n, imagery.edit_instruction) } else { t_string!(i18n, imagery.prompt_used) }}
                    </summary>
                    <p class="mt-1 text-[11px] leading-relaxed text-gray-600 dark:text-gray-300 selectable">{prompt.clone()}</p>
                </details>
            })}
            <div class="pl-1">{usage}</div>
        </div>
    }
    .into_any()
}
