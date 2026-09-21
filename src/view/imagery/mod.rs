//! 图片工作台(一级入口)。接图片生成模型的接口出图,并且围绕**一条和 AI 的持续对话**来改图。
//!
//!   左:画板列表(一个画板 = 一个主题的一组图:参考图 + 对话 + 生成过和导入的每一张)
//!   中:当前选中的图(大图)+ 这条线上所有图的胶片条;采用 / 送去建模 / 导出
//!   右:对话——第一句话出图;之后每句话要么改当前这张、要么重新出一批、要么只是回答
//!
//! 图不一定是生成的:自己的图(实拍、草图、找来的参考)可以导入——「导入图片」按钮、直接拖进窗口、或者 Ctrl+V。
//! 导入的图和生成的图同等地位:能选中、采用、导出、送去建模,也能让 AI 接着改。
//! 拖到哪、粘到哪决定它的去向:落在画板这边 = 导入成画板上的一张图;落在对话栏 = 只贴到这句话上当参考。
//!
//! 和建模工作室同构,思路见 docs/adr/0005-image-studio.md。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::design::CadDesign;
use pp_common::imagery::{
    is_importable_image, ImageAspect, ImageBoard, ImageBoardDetail, ImageBoardSummary, ImageImportResult, ImageMessage, ImageMsgKind,
    ImageProviderInfo, ImagePurpose, ImageTurnResult, ImageVersion, IMPORT_EXTENSIONS,
};
use pp_common::Asset;

use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::{current_locale, localize_backend_err};
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::{AppState, DroppedFiles, Handoff, Route, SettingsSection};
use crate::theme::{get_pref, set_pref};
use crate::ui::{
    basics, copy_entry, image_entries, item, separator, Badge, Button, ButtonVariant, Dialog, DialogFooter, DropPanel, EmptyState, IconButton, Segmented, Tone,
};

/// 一轮最多涉及 3 张图(当前选中的那张也算一张)——和后端的 MAX_TURN_IMAGES 一致
const MAX_TURN_IMAGES: usize = 3;
/// 对话输入框的 id:在它里面按 Ctrl+V 粘贴图片 = 贴到这句话上,而不是导入到画板
const COMPOSER_ID: &str = "imagery-composer";

/// 正拖着文件悬在窗口上时,光标底下那块地方会怎么处理它
#[derive(Copy, Clone, PartialEq, Eq)]
enum DropZone {
    /// 导入成画板上的图
    Import,
    /// 贴到这句话上当参考
    Attach,
    /// 拖的不是图片
    Reject,
}

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
    // 正在导入(大照片要解码、缩放、重新编码,一批可能要几秒)
    let importing = RwSignal::new(false);
    let count = RwSignal::new(get_pref("imagery_count").and_then(|c| c.parse::<u32>().ok()).unwrap_or(2));
    let delete_dialog = RwSignal::new(false);
    // 要删的是哪个画板 / 要拿掉的是哪张图(`None` = 打开着的那个 / 当前这张)。右键菜单可以指向列表里任意一个
    let delete_target = RwSignal::new(None::<String>);
    let remove_dialog = RwSignal::new(false);
    let remove_target = RwSignal::new(None::<String>);
    let renaming = RwSignal::new(None::<String>);
    // 右键菜单点了「重命名」、但那个画板还没打开:先打开,载入之后再进入改名
    let rename_pending = StoredValue::new(None::<String>);

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
                    let rename_now = (rename_pending.get_value().as_deref() == Some(d.board.id.as_str())).then(|| d.board.name.clone());
                    rename_pending.set_value(None);
                    board.set(Some(d.board));
                    renaming.set(rename_now);
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
    let attach_room_now = move || MAX_TURN_IMAGES.saturating_sub(pending.with_untracked(Vec::len) + current.with_untracked(|c| c.is_some() as usize));
    // 把这几个文件贴到这句话上当参考(「贴一张图」按钮选的、或者拖到对话栏里的)
    let attach_paths = move |paths: Vec<String>| {
        let Some(id) = board_id() else { return };
        let room = attach_room_now();
        if room < paths.len() {
            state.notify_info(td_string!(current_locale(), imagery.drop_attach_full));
        }
        let take: Vec<String> = paths.into_iter().take(room).collect();
        spawn_local(async move {
            for path in take {
                match ipc::call::<_, Asset>(cmd::BOARD_ADD_IMAGE, &serde_json::json!({ "id": id, "path": path })).await {
                    Ok(asset) => pending.update(|l| l.push(asset)),
                    Err(e) => state.notify_error(e),
                }
            }
        });
    };
    let attach = move || {
        if attach_room_now() == 0 {
            return;
        }
        spawn_local(async move {
            if let Some(path) = ipc::pick_file("Image", &IMPORT_EXTENSIONS).await {
                attach_paths(vec![path]);
            }
        });
    };

    // ---- 导入自己的图 ----
    // 导入 / 粘贴的结果落到界面上:新图排到胶片条最前面,第一张成为当前图;导不进来的逐个说明
    let apply_import = move |res: ImageImportResult| {
        let l = current_locale();
        let imported = res.versions.len();
        versions.update(|list| {
            for v in res.versions.into_iter().rev() {
                list.insert(0, v);
            }
        });
        if let Some(msg) = res.message {
            messages.update(|list| list.push(msg));
        }
        board.set(Some(res.board));
        reload_list();
        state.reload_project_facts();
        if imported > 0 {
            state.notify_info(td_string!(l, imagery.imported_toast, count = imported).to_string());
        }
        if !res.failed.is_empty() {
            // 最多念三个名字,别让一条提示占满屏
            let detail = res
                .failed
                .iter()
                .take(3)
                .map(|f| {
                    let why = localize_backend_err(&f.reason);
                    if f.name.is_empty() { why } else { format!("{} — {why}", f.name) }
                })
                .collect::<Vec<_>>()
                .join(";");
            state.notify_error(td_string!(l, imagery.import_failed, count = res.failed.len(), detail = detail).to_string());
        }
    };
    // 导入总得有个画板接着:一个都没打开(刚装好、或者全删了)就先建一个
    let ensure_board = move || async move {
        if let Some(id) = board_id() {
            return Some(id);
        }
        let args = serde_json::json!({ "purpose": ImagePurpose::ModelRef, "project_id": null, "name": null });
        match ipc::call::<_, ImageBoard>(cmd::BOARD_CREATE, &args).await {
            Ok(b) => {
                let id = b.id.clone();
                set_pref("imagery_board", &id);
                messages.set(Vec::new());
                versions.set(Vec::new());
                pending.set(Vec::new());
                board.set(Some(b));
                Some(id)
            }
            Err(e) => {
                state.notify_error(e);
                None
            }
        }
    };
    // `command`:从文件导入(带路径),或者从剪贴板导入
    let run_import = move |command: &'static str, paths: Option<Vec<String>>| {
        if busy.get_untracked() || importing.get_untracked() {
            state.notify_info(td_string!(current_locale(), imagery.import_busy));
            return;
        }
        importing.set(true);
        spawn_local(async move {
            if let Some(id) = ensure_board().await {
                let args = match paths {
                    Some(paths) => serde_json::json!({ "id": id, "paths": paths }),
                    None => serde_json::json!({ "id": id }),
                };
                match ipc::call::<_, ImageImportResult>(command, &args).await {
                    Ok(res) => apply_import(res),
                    Err(e) => state.notify_error(e),
                }
            }
            importing.set(false);
        });
    };
    let import_paths = move |paths: Vec<String>| {
        if !paths.is_empty() {
            run_import(cmd::BOARD_IMPORT_IMAGES, Some(paths));
        }
    };
    let import_click = move || {
        spawn_local(async move {
            import_paths(ipc::pick_files("Image", &IMPORT_EXTENSIONS).await);
        });
    };
    let paste_images = move || run_import(cmd::BOARD_PASTE_IMAGES, None);
    // 在输入框里粘贴图片:贴到这句话上当参考
    let paste_reference = move || {
        let Some(id) = board_id() else { return };
        let room = attach_room_now();
        if room == 0 {
            state.notify_info(td_string!(current_locale(), imagery.drop_attach_full));
            return;
        }
        spawn_local(async move {
            match ipc::call::<_, Vec<Asset>>(cmd::BOARD_PASTE_REFERENCE, &serde_json::json!({ "id": id, "max": room })).await {
                Ok(list) => pending.update(|l| l.extend(list)),
                Err(e) => state.notify_error(e),
            }
        });
    };

    // ---- 拖进来的文件:落在对话栏 = 贴到这句话上;落在别处 = 导入到画板 ----
    let chat_ref = NodeRef::<leptos::html::Aside>::new();
    let over_chat = move |x: f64, y: f64| {
        // 没打开画板时对话栏被空状态盖着:整块都算「导入」
        board.with_untracked(Option::is_some)
            && chat_ref.get_untracked().is_some_and(|el| {
                let r = el.get_bounding_client_rect();
                x >= r.left() && x <= r.right() && y >= r.top() && y <= r.bottom()
            })
    };
    let drop_zone = Memo::new(move |_| {
        state.file_drag.with(|drag| {
            let drag = drag.as_ref()?;
            if !drag.paths.iter().any(|p| is_importable_image(p)) {
                return Some(DropZone::Reject);
            }
            Some(if over_chat(drag.x, drag.y) { DropZone::Attach } else { DropZone::Import })
        })
    });
    state.accept_file_drops(move |dropped: DroppedFiles| {
        let pictures: Vec<String> = dropped.paths.iter().filter(|p| is_importable_image(p)).cloned().collect();
        if pictures.is_empty() {
            state.notify_info(td_string!(current_locale(), imagery.drop_reject));
        } else if over_chat(dropped.x, dropped.y) {
            attach_paths(pictures);
        } else {
            // 整批交给后端:混在里面的不是图片的文件,它会逐个说明为什么没导入
            import_paths(dropped.paths);
        }
    });
    // Ctrl+V:剪贴板里是图片(截图、浏览器里「复制图片」、资源管理器里复制的图片文件)才接手,文字照常粘贴。
    // 图片本身由后端去读剪贴板——WebView 里只能拿到一个没有路径的 File。
    let paste_listener = window_event_listener(leptos::ev::paste, move |ev: web_sys::Event| {
        use wasm_bindgen::{JsCast, JsValue};
        // web-sys 的 ClipboardEvent 还算「不稳定接口」(要加编译开关才有),这里只读 `clipboardData.types`,直接按属性取
        let types = js_sys::Reflect::get(&ev, &JsValue::from_str("clipboardData"))
            .and_then(|data| js_sys::Reflect::get(&data, &JsValue::from_str("types")))
            .map(|types| js_sys::Array::from(&types))
            .unwrap_or_default();
        let has = |name: &str| types.iter().any(|t| t.as_string().as_deref() == Some(name));
        if !has("Files") {
            return;
        }
        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
        let in_composer = target.as_ref().is_some_and(|el| el.id() == COMPOSER_ID);
        let in_editor = target.as_ref().is_some_and(|el| matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA"));
        // 输入框里:既有字又有图(从网页、文档里复制的一段)按文字粘;别的输入框(改名)不收图
        if in_editor && (has("text/plain") || !in_composer) {
            return;
        }
        ev.prevent_default();
        if in_composer {
            paste_reference();
        } else {
            paste_images();
        }
    });
    on_cleanup(move || paste_listener.remove());
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
    let adopt_version = move |v: ImageVersion| {
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
    let toggle_adopt = move || {
        if let Some(v) = current.get_untracked() {
            adopt_version(v);
        }
    };
    let remove_image = move || {
        remove_dialog.set(false);
        let Some(version_id) = remove_target.get_untracked().or_else(|| current.get_untracked().map(|v| v.id)) else { return };
        remove_target.set(None);
        spawn_local(async move {
            match ipc::call::<_, ImageBoard>(cmd::BOARD_REMOVE_IMAGE, &serde_json::json!({ "version_id": version_id })).await {
                Ok(b) => {
                    versions.update(|l| l.retain(|x| x.id != version_id));
                    board.set(Some(b));
                    reload_list();
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    // 送去建模:用这张图新建一个设计,跳到「建模」打开它
    let design_from = move |version_id: String| {
        spawn_local(async move {
            match ipc::call::<_, CadDesign>(cmd::BOARD_TO_DESIGN, &serde_json::json!({ "version_id": version_id })).await {
                Ok(d) => {
                    set_pref("studio_design", &d.id);
                    state.go(Route::Studio);
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let to_design = move || {
        if let Some(v) = current.get_untracked() {
            design_from(v.id);
        }
    };
    let export_version = move |version_id: String| {
        let name = board.with_untracked(|b| b.as_ref().map(|b| b.name.clone())).unwrap_or_else(|| "image".into());
        spawn_local(async move {
            let Some(dest) = ipc::pick_save_path("Export", &format!("{name}.jpg"), "jpg").await else {
                return;
            };
            match ipc::call_unit(cmd::BOARD_EXPORT, &serde_json::json!({ "version_id": version_id, "dest_path": dest })).await {
                Ok(()) => state.notify_info(td_string!(current_locale(), lab.exported)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let export = move || {
        if let Some(v) = current.get_untracked() {
            export_version(v.id);
        }
    };
    // 右键一张图(胶片条、对话里、中间的大图):对**这一张**操作,不用先把它选成当前
    let version_menu = move |ev: web_sys::MouseEvent, version_id: String| {
        let Some(v) = versions.with_untracked(|l| l.iter().find(|v| v.id == version_id).cloned()) else {
            state.open_menu(&ev, Vec::new());
            return;
        };
        let l = current_locale();
        let is_current = current.with_untracked(|c| c.as_ref().is_some_and(|c| c.id == v.id));
        let working = busy.get_untracked();
        let (select_id, design_id, export_id, remove_id, adopt) = (v.id.clone(), v.id.clone(), v.id.clone(), v.id.clone(), v.clone());
        let mut entries = vec![
            item(td_string!(l, imagery.menu_select), IconKind::Check, move || select(select_id.clone())).disabled_if(is_current),
            item(if v.adopted { td_string!(l, imagery.unadopt) } else { td_string!(l, imagery.adopt) }, IconKind::Star, move || adopt_version(adopt.clone())),
            item(td_string!(l, imagery.to_design), IconKind::Box, move || design_from(design_id.clone())).disabled_if(working),
            item(td_string!(l, imagery.menu_export), IconKind::Download, move || export_version(export_id.clone())),
            separator(),
        ];
        entries.extend(image_entries(state, v.asset_id.clone()));
        if !v.prompt.trim().is_empty() {
            entries.push(copy_entry(state, td_string!(l, menu.copy_prompt), v.prompt.clone()));
        }
        entries.push(separator());
        entries.push(
            item(td_string!(l, imagery.remove_image), IconKind::Trash, move || {
                remove_target.set(Some(remove_id.clone()));
                remove_dialog.set(true);
            })
            .danger()
            .disabled_if(working),
        );
        state.open_menu(&ev, entries);
    };
    let version_menu = Callback::new(move |(ev, id): (web_sys::MouseEvent, String)| version_menu(ev, id));
    // 右键大图周围的空白处:往这个画板里放图
    let viewer_menu = move |ev: web_sys::MouseEvent| {
        let l = current_locale();
        let working = busy.get_untracked() || importing.get_untracked();
        state.open_menu(
            &ev,
            vec![
                item(td_string!(l, imagery.import), IconKind::Upload, import_click).disabled_if(working),
                item(td_string!(l, imagery.paste_image), IconKind::Clipboard, paste_images).disabled_if(working),
            ],
        );
    };
    // ---- 右键菜单用的:对列表里任意一个画板操作 ----
    let start_rename = move |id: String| {
        if board_id().as_deref() == Some(id.as_str()) {
            renaming.set(board.with_untracked(|b| b.as_ref().map(|b| b.name.clone())));
        } else {
            rename_pending.set_value(Some(id.clone()));
            open(id);
        }
    };
    let ask_delete = move |id: String| {
        delete_target.set(Some(id));
        delete_dialog.set(true);
    };
    let delete_name = move || {
        let id = delete_target.get().or_else(|| board.with(|b| b.as_ref().map(|b| b.id.clone())));
        id.and_then(|id| boards.with(|l| l.iter().find(|b| b.id == id).map(|b| b.name.clone()))).unwrap_or_default()
    };
    // 改名框一出现就拿到焦点并全选(从右键菜单进来时,没有任何点击会落在它上面)
    let rename_input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if renaming.with(Option::is_some) {
            if let Some(input) = rename_input.get() {
                let _ = input.focus();
                input.select();
            }
        }
    });
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
        // 页面被切走时,正聚焦的输入框会在移除的瞬间收到 blur——那时这些信号已经销毁了(直接读会 panic)
        let Some(Some(name)) = renaming.try_get_untracked() else { return };
        let Some(id) = board_id() else { return };
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
        let Some(id) = delete_target.get_untracked().or_else(board_id) else { return };
        let was_open = board_id().as_deref() == Some(id.as_str());
        delete_dialog.set(false);
        delete_target.set(None);
        spawn_local(async move {
            match ipc::call_unit(cmd::BOARD_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    boards.update(|l| l.retain(|b| b.id != id));
                    state.reload_project_facts();
                    // 删的是列表里别的画板:打开着的这个不受影响
                    if !was_open {
                        return;
                    }
                    board.set(None);
                    messages.set(Vec::new());
                    versions.set(Vec::new());
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
                            <button type="button" class="text-amber-600 dark:text-amber-400 hover:underline" on:click=move |_| state.go_settings(SettingsSection::Keys)>
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
                            <button type="button" class="text-amber-600 dark:text-amber-400 hover:underline" on:click=move |_| state.go_settings(SettingsSection::Keys)>
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
                            <BoardRow state=state row=row board=board on_open=open on_rename=start_rename on_delete=ask_delete/>
                        </For>
                    </div>
                </aside>

                <div class="relative flex-1 min-w-0 h-full flex">
                    <Show when=move || board.with(Option::is_none)>
                        <div class="absolute inset-0 z-10 bg-gray-50 dark:bg-gray-900">
                            <EmptyState icon=IconKind::Image title=move || t_string!(i18n, imagery.empty_title) hint=move || t_string!(i18n, imagery.empty_hint)>
                                <div class="flex items-center justify-center gap-2">
                                    <Button icon=IconKind::Plus on_click=move || new_board(ImagePurpose::ModelRef)>{move || t_string!(i18n, imagery.new_board)}</Button>
                                    <Button variant=ButtonVariant::Secondary icon=IconKind::Upload disabled=Signal::derive(move || importing.get()) on_click=import_click>
                                        {move || t_string!(i18n, imagery.import)}
                                    </Button>
                                </div>
                            </EmptyState>
                        </div>
                    </Show>
                    // 正拖着文件悬在窗口上:告诉用户松手之后会发生什么(两块地方去向不同)
                    <Show when=move || drop_zone.get().is_some()>
                        <DropHint zone=drop_zone has_board=Signal::derive(move || board.with(Option::is_some)) chat=chat_ref/>
                    </Show>

                    // ---- 中:当前的图 + 胶片条 ----
                    <section class="flex-1 min-w-0 h-full flex flex-col">
                        <div class="shrink-0 h-11 px-3 flex items-center gap-3 border-b border-gray-200 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap overflow-hidden">
                            {move || match renaming.get() {
                                Some(_) => view! {
                                    <input
                                        type="text"
                                        node_ref=rename_input
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
                            <IconButton
                                icon=IconKind::Trash
                                label=move || t_string!(i18n, imagery.delete_board)
                                on_click=move || {
                                    delete_target.set(None);
                                    delete_dialog.set(true);
                                }
                            />
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
                        <div class="relative flex-1 min-h-0 bg-gray-100 dark:bg-gray-900 flex items-center justify-center p-4" on:contextmenu=viewer_menu>
                            {move || match current.get() {
                                Some(v) => {
                                    let vid = v.id.clone();
                                    view! {
                                        <img
                                            src=ipc::asset_url(&v.asset_id)
                                            class="max-w-full max-h-full object-contain rounded-lg shadow-lg"
                                            draggable="false"
                                            on:contextmenu=move |ev| version_menu.run((ev, vid.clone()))
                                        />
                                    }.into_any()
                                }
                                None => view! {
                                    <EmptyState icon=IconKind::Image title=move || t_string!(i18n, imagery.viewer_empty) hint=move || t_string!(i18n, imagery.viewer_empty_hint)>
                                        <Button
                                            variant=ButtonVariant::Secondary
                                            icon=IconKind::Upload
                                            disabled=Signal::derive(move || busy.get() || importing.get())
                                            on_click=import_click
                                        >
                                            {move || t_string!(i18n, imagery.import)}
                                        </Button>
                                    </EmptyState>
                                }.into_any(),
                            }}
                            <Show when=move || importing.get()>
                                <div class="absolute top-3 left-1/2 -translate-x-1/2 px-3 py-1.5 rounded-full bg-white/95 dark:bg-gray-800/95 shadow text-xs text-gray-600 dark:text-gray-300">
                                    {move || t_string!(i18n, imagery.importing)}
                                </div>
                            </Show>
                            <Show when=move || current.with(|v| v.as_ref().is_some_and(|v| v.adopted))>
                                <div class="absolute left-3 top-3"><Badge tone=Tone::Green>{move || t_string!(i18n, imagery.adopted)}</Badge></div>
                            </Show>
                            <div class="absolute right-3 bottom-3 flex items-center gap-1.5">
                                <div class="rounded-lg bg-white/90 dark:bg-gray-800/90">
                                    <IconButton
                                        icon=IconKind::Trash
                                        label=move || t_string!(i18n, imagery.remove_image)
                                        disabled=no_image
                                        on_click=move || {
                                            remove_target.set(None);
                                            remove_dialog.set(true);
                                        }
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
                                <button
                                    type="button"
                                    class="shrink-0 w-[4.5rem] h-[4.5rem] rounded-lg border-2 border-dashed border-gray-300 dark:border-gray-600 \
                                           text-gray-400 hover:border-brand hover:text-brand flex flex-col items-center justify-center gap-1 \
                                           transition-colors disabled:opacity-50 disabled:pointer-events-none"
                                    title=move || t_string!(i18n, imagery.import_tip)
                                    disabled=move || busy.get() || importing.get()
                                    on:click=move |_| import_click()
                                >
                                    <Icon kind=IconKind::Upload class="w-4 h-4"/>
                                    <span class="text-[11px]">{move || t_string!(i18n, imagery.import_short)}</span>
                                </button>
                                <For each=move || versions.get() key=|v| (v.id.clone(), v.adopted) let:v>
                                    <FilmCell version=v current=current on_select=select on_menu=version_menu/>
                                </For>
                            </div>
                        </Show>
                    </section>

                    // ---- 右:对话 ----
                    <aside node_ref=chat_ref class="w-[24rem] shrink-0 h-full flex flex-col border-l border-gray-200 dark:border-gray-700">
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
                                <MessageView state=state msg=m versions=versions current=current on_select=select on_menu=version_menu/>
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
                                id=COMPOSER_ID
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
                <p class="text-sm font-medium text-gray-900 dark:text-gray-100 truncate">{delete_name}</p>
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
fn BoardRow(
    state: AppState,
    row: ImageBoardSummary,
    board: RwSignal<Option<ImageBoard>>,
    #[prop(into)] on_open: Callback<(String,)>,
    #[prop(into)] on_rename: Callback<(String,)>,
    #[prop(into)] on_delete: Callback<(String,)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(row.id.clone());
    let name = StoredValue::new(row.name.clone());
    let menu = move |ev: web_sys::MouseEvent| {
        let l = current_locale();
        let (open_id, rename_id, delete_id) = (id.get_value(), id.get_value(), id.get_value());
        state.open_menu(
            &ev,
            vec![
                item(td_string!(l, menu.open), IconKind::Image, move || on_open.run((open_id.clone(),))),
                item(td_string!(l, menu.rename), IconKind::Pencil, move || on_rename.run((rename_id.clone(),))),
                copy_entry(state, td_string!(l, menu.copy_name), name.get_value()),
                separator(),
                item(td_string!(l, menu.delete), IconKind::Trash, move || on_delete.run((delete_id.clone(),))).danger(),
            ],
        );
    };
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
            on:contextmenu=menu
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
fn FilmCell(
    version: ImageVersion,
    current: Memo<Option<ImageVersion>>,
    #[prop(into)] on_select: Callback<(String,)>,
    on_menu: Callback<(web_sys::MouseEvent, String)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(version.id.clone());
    let active = move || current.with(|c| c.as_ref().is_some_and(|c| id.with_value(|id| &c.id == id)));
    view! {
        <button
            type="button"
            class="relative shrink-0 w-[4.5rem] h-[4.5rem] rounded-lg overflow-hidden border-2 transition-colors bg-gray-100 dark:bg-gray-800"
            class=("border-brand", active)
            class=("border-transparent", move || !active())
            title=(!version.prompt.is_empty()).then(|| version.prompt.clone())
            on:click=move |_| on_select.run((id.get_value(),))
            on:contextmenu=move |ev| on_menu.run((ev, id.get_value()))
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
            // 自己导入的(不是模型画的)
            {(version.mode == "import").then(|| view! {
                <span
                    class="absolute right-0.5 bottom-0.5 w-4 h-4 rounded-full bg-sky-500 text-white flex items-center justify-center"
                    title=move || t_string!(i18n, imagery.imported_badge)
                >
                    <Icon kind=IconKind::Upload class="w-2.5 h-2.5"/>
                </span>
            })}
        </button>
    }
}

#[component]
fn MessageView(
    state: AppState,
    msg: ImageMessage,
    versions: RwSignal<Vec<ImageVersion>>,
    current: Memo<Option<ImageVersion>>,
    #[prop(into)] on_select: Callback<(String,)>,
    on_menu: Callback<(web_sys::MouseEvent, String)>,
) -> impl IntoView {
    let i18n = use_i18n();
    // 右键一条消息:复制这段话(点在图上的话,前面还有图片的那几项)
    let said = StoredValue::new(msg.content.clone());
    let text_menu = move |ev: web_sys::MouseEvent| {
        let mut entries = basics(state, &ev);
        if !said.with_value(|s| s.trim().is_empty()) {
            entries.push(copy_entry(state, td_string!(current_locale(), menu.copy_text), said.get_value()));
        }
        state.open_menu(&ev, entries);
    };
    // 用户导入的一批图:靠右(是用户做的事),点一张就选中它
    if msg.from_user && msg.kind == ImageMsgKind::Images {
        let ids = msg.extra.version_ids.clone();
        let total = ids.len();
        let batch = StoredValue::new(ids.clone());
        let all_removed = move || versions.with(|l| batch.with_value(|ids| !ids.iter().any(|id| l.iter().any(|v| &v.id == id))));
        return view! {
            <div class="flex flex-col items-end gap-1">
                <div class="flex flex-wrap justify-end gap-1.5 max-w-[92%]">
                    {ids.iter().map(|vid| {
                        let vid = StoredValue::new(vid.clone());
                        let asset = move || versions.with(|l| vid.with_value(|id| l.iter().find(|v| &v.id == id).map(|v| v.asset_id.clone())));
                        let active = move || current.with(|c| c.as_ref().is_some_and(|c| vid.with_value(|id| &c.id == id)));
                        view! {
                            <Show when=move || asset().is_some()>
                                <button
                                    type="button"
                                    class="w-16 h-16 rounded-lg overflow-hidden border-2 transition-colors bg-gray-100 dark:bg-gray-900"
                                    class=("border-brand", active)
                                    class=("border-transparent", move || !active())
                                    on:click=move |_| on_select.run((vid.get_value(),))
                                    on:contextmenu=move |ev| on_menu.run((ev, vid.get_value()))
                                >
                                    {move || asset().map(|a| view! { <img src=ipc::asset_url(&a) class="w-full h-full object-cover" draggable="false"/> })}
                                </button>
                            </Show>
                        }
                    }).collect_view()}
                </div>
                <p class="pr-1 text-[11px] text-gray-400">
                    {move || if all_removed() {
                        t_string!(i18n, imagery.images_removed).to_string()
                    } else {
                        t_string!(i18n, imagery.imported_caption, count = total).to_string()
                    }}
                </p>
            </div>
        }
        .into_any();
    }
    if msg.from_user {
        return view! {
            <div class="flex flex-col items-end gap-1" on:contextmenu=text_menu>
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
        <div class="space-y-1.5" on:contextmenu=text_menu>
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
                                    on:contextmenu=move |ev| on_menu.run((ev, vid.get_value()))
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

/// 正拖着文件悬在窗口上时盖在工作台上的提示:画板这边 = 导入,对话栏 = 贴到这句话上;光标在哪边,哪边亮。
/// 整层 `pointer-events-none`——它只是提示,真正的落点由 Tauri 的拖放事件带来的坐标决定。
#[component]
fn DropHint(zone: Memo<Option<DropZone>>, has_board: Signal<bool>, chat: NodeRef<leptos::html::Aside>) -> impl IntoView {
    let i18n = use_i18n();
    // 对话栏有多宽,提示就从右边让出多宽(没打开画板时整块都是「导入」)
    let chat_width = move || {
        if !has_board.get() {
            return 0.0;
        }
        chat.get().map(|el| el.get_bounding_client_rect().width()).unwrap_or(0.0)
    };
    view! {
        <div class="absolute inset-0 z-20 flex pointer-events-none">
            <div class="flex-1 min-w-0 p-3">
                <DropPanel
                    active=Signal::derive(move || zone.get() == Some(DropZone::Import))
                    icon=IconKind::Upload
                    text=Signal::derive(move || match (zone.get(), has_board.get()) {
                        (Some(DropZone::Reject), _) => t_string!(i18n, imagery.drop_reject).to_string(),
                        (_, true) => t_string!(i18n, imagery.drop_import).to_string(),
                        (_, false) => t_string!(i18n, imagery.drop_import_new).to_string(),
                    })
                />
            </div>
            <Show when=move || has_board.get() && zone.get() != Some(DropZone::Reject)>
                <div class="shrink-0 p-3 pl-0" style:width=move || format!("{}px", chat_width())>
                    <DropPanel
                        active=Signal::derive(move || zone.get() == Some(DropZone::Attach))
                        icon=IconKind::Image
                        text=Signal::derive(move || t_string!(i18n, imagery.drop_attach).to_string())
                    />
                </div>
            </Show>
        </div>
    }
}
