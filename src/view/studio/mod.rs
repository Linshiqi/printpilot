//! 建模工作室(一级入口)。看图 → build123d 代码建模,围绕**一条和 AI 的持续对话**来改模型。
//!
//!   左:设计列表(一个设计 = 一个零件:参考图 + 规格 + 对话 + 版本树)
//!   中:3D 视图(棱线、点选位置、导出)+ 代码编辑器
//!   右:对话(主线)· 参数(不经过模型的微调)· 版本
//!
//! 三种改法的代价从低到高:改参数(本机重建,约 0.2 秒,不花钱)→ 对话修改(只动相关的特征段)→ 改规格重新生成。
//! 选中旧版本再说话 = 从那一版分叉;每次对话修改都可以一键撤销。设计与决策见 docs/adr/0004-modeling-studio.md。

mod chat;
mod side;
mod spec_editor;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::cad::{CadEngineInfo, CadPick, CadVersion, DesignSpec};
use pp_common::design::{CadDesign, CadDesignDetail, CadDesignSummary, CadMessage, CadTier, CadTurnResult};
use pp_common::Asset;

use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::current_locale;
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::theme::{get_pref, set_pref, Theme};
use crate::ui::{Badge, Button, ButtonVariant, Card, Dialog, DialogFooter, EmptyState, IconButton, Segmented, Toggle, Tone};
use crate::utils::fmt_mm;
use crate::viewer3d::{self, Pick, PickHandler};
use chat::{ChatActions, ChatPane};
use side::{changed_text, MetricsCard, ParamsCard, ReportCard};
use spec_editor::{SpecDraft, SpecEditor, SpecProblem};

const VIEWER_ID: &str = "studio-viewer";
pub(super) const BUILD_VOLUME: [f64; 3] = [256.0, 256.0, 256.0];
/// 一句话里最多贴几张图
const MAX_IMAGES: usize = 4;
/// 复核截图的边长(像素);列表缩略图的边长
const REVIEW_SHOT_PX: u32 = 768;
const THUMB_PX: u32 = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Chat,
    Params,
    Versions,
}

fn source_name(l: Locale, source: &str) -> &'static str {
    match source {
        "generate" | "repair" => td_string!(l, cad.src_generate),
        "param" => td_string!(l, cad.src_param),
        "edit" => td_string!(l, cad.src_edit),
        _ => td_string!(l, cad.src_manual),
    }
}

fn source_tone(source: &str) -> Tone {
    match source {
        "generate" | "repair" => Tone::Brand,
        "edit" => Tone::Amber,
        "param" => Tone::Green,
        _ => Tone::Neutral,
    }
}

#[component]
pub fn StudioView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();

    // ---- 状态 ----
    let engine = RwSignal::new(None::<CadEngineInfo>); // None = 还在检查
    let installing = RwSignal::new(false);
    let designs = RwSignal::new(Vec::<CadDesignSummary>::new());
    let design = RwSignal::new(None::<CadDesign>);
    let messages = RwSignal::new(Vec::<CadMessage>::new());
    let versions = RwSignal::new(Vec::<CadVersion>::new());
    let busy = RwSignal::new(false);
    let pane = RwSignal::new(Pane::Chat);

    let text = RwSignal::new(String::new());
    let pending_images = RwSignal::new(Vec::<Asset>::new());
    let tier = RwSignal::new(get_pref("studio_tier").map(|t| CadTier::parse(&t)).unwrap_or_default());
    let code = RwSignal::new(String::new());
    let show_code = RwSignal::new(false);
    let spec_dialog = RwSignal::new(false);
    let spec_draft = RwSignal::new(None::<SpecDraft>);
    let delete_dialog = RwSignal::new(false);
    let renaming = RwSignal::new(None::<String>);

    let viewer_ready = RwSignal::new(false);
    let viewer_error = RwSignal::new(None::<String>);
    let edges = RwSignal::new(true);
    let show_bed = RwSignal::new(true);
    let pick_mode = RwSignal::new(false);
    let picked = RwSignal::new(None::<Pick>);
    let pick_handler = StoredValue::new_local(None::<PickHandler>);
    // 上一次加载进视图的版本:新版本如果是它的子版本,镜头跟着零件走而不是重新取景
    let loaded_id = StoredValue::new(None::<String>);

    let demo = move || state.app_info.with(|i| i.as_ref().is_some_and(|i| i.demo_mode));
    let engine_ok = move || engine.with(|e| e.as_ref().is_some_and(|e| e.available));
    let design_id = move || design.with_untracked(|d| d.as_ref().map(|d| d.id.clone()));
    // 视图里显示的版本 = 设计的「当前版本」
    let current = Memo::new(move |_| {
        let id = design.with(|d| d.as_ref().and_then(|d| d.current_version_id.clone()))?;
        versions.with(|list| list.iter().find(|v| v.id == id).cloned())
    });
    let has_model = Signal::derive(move || current.with(Option::is_some));

    // ---- 引擎:正式版的安装包里带着引擎包,第一次用到时自动解开 ----
    let check_engine = move || {
        engine.set(None);
        spawn_local(async move {
            let mut info = ipc::call_no_args::<CadEngineInfo>(cmd::CAD_ENGINE_INFO).await;
            if info.as_ref().is_ok_and(|i| i.needs_install) {
                state.cad_engine_progress.set(None);
                engine.set(info.clone().ok());
                installing.set(true);
                let installed = ipc::call_unit_no_args(cmd::CAD_ENGINE_INSTALL).await;
                installing.set(false);
                state.cad_engine_progress.set(None);
                info = match installed {
                    Ok(()) => ipc::call_no_args::<CadEngineInfo>(cmd::CAD_ENGINE_INFO).await,
                    Err(e) => Ok(CadEngineInfo {
                        bundled: true,
                        problem: e,
                        ..Default::default()
                    }),
                };
            }
            match info {
                Ok(info) => engine.set(Some(info)),
                Err(e) => {
                    state.notify_error(e);
                    engine.set(Some(CadEngineInfo::default()));
                }
            }
        });
    };
    check_engine();

    // ---- 设计 ----
    let reload_list = move || {
        spawn_local(async move {
            match ipc::call_no_args::<Vec<CadDesignSummary>>(cmd::DESIGN_LIST).await {
                Ok(list) => designs.set(list),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let show_detail = move |d: CadDesignDetail| {
        set_pref("studio_design", &d.design.id);
        let current_code = d
            .design
            .current_version_id
            .as_ref()
            .and_then(|id| d.versions.iter().find(|v| &v.id == id))
            .map(|v| v.code.clone())
            .unwrap_or_default();
        code.set(current_code);
        text.set(String::new());
        pending_images.set(Vec::new());
        pick_mode.set(false);
        loaded_id.set_value(None);
        messages.set(d.messages);
        versions.set(d.versions);
        design.set(Some(d.design));
        pane.set(Pane::Chat);
    };
    let open = move |id: String| {
        spawn_local(async move {
            match ipc::call::<_, CadDesignDetail>(cmd::DESIGN_GET, &serde_json::json!({ "id": id })).await {
                Ok(d) => show_detail(d),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let new_design = move || {
        spawn_local(async move {
            match ipc::call::<_, CadDesign>(cmd::DESIGN_CREATE, &serde_json::json!({ "name": null, "project_id": null })).await {
                Ok(d) => {
                    reload_list();
                    open(d.id);
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    // 进来先列出设计,并回到上次打开的那个
    spawn_local(async move {
        match ipc::call_no_args::<Vec<CadDesignSummary>>(cmd::DESIGN_LIST).await {
            Ok(list) => {
                let last = get_pref("studio_design");
                let pick = list.iter().find(|d| Some(&d.id) == last.as_ref()).or(list.first()).map(|d| d.id.clone());
                designs.set(list);
                if let Some(id) = pick {
                    open(id);
                }
            }
            Err(e) => state.notify_error(e),
        }
    });

    // 任何「往时间线上加东西」的操作的收尾:新消息、(可能有的)新版本、更新后的设计
    let apply_turn = move |res: CadTurnResult| {
        if let Some(v) = res.version {
            state.notify_info(td_string!(current_locale(), lab.done_in, ms = v.elapsed_ms).to_string());
            code.set(v.code.clone());
            versions.update(|list| list.insert(0, v));
        }
        messages.update(|list| list.extend(res.messages));
        design.set(Some(res.design));
        reload_list();
    };

    // 所有「调后端、可能要等很久」的操作走同一条路:上锁 → 清进度 → 调用 → 解锁
    let run_job = move |job: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        state.cad_progress.set(None);
        spawn_local(async move {
            if let Err(e) = job.await {
                state.notify_error(e);
            }
            busy.set(false);
            state.cad_progress.set(None);
        });
    };

    // ---- 对话 ----
    let send_text = move |said: String| {
        let Some(id) = design_id() else { return };
        let images: Vec<String> = pending_images.with_untracked(|l| l.iter().map(|a| a.id.clone()).collect());
        if said.trim().is_empty() && images.is_empty() {
            return;
        }
        let pick = picked.get_untracked().map(|p| CadPick {
            point: p.point,
            normal: Some(p.normal),
        });
        let args = serde_json::json!({ "id": id, "text": said, "pick": pick, "image_asset_ids": images, "tier": tier.get_untracked() });
        pane.set(Pane::Chat);
        run_job(Box::pin(async move {
            // 失败时什么都没入库:输入框里的话留着,改好配置再发一次
            let res = ipc::call::<_, CadTurnResult>(cmd::DESIGN_SEND, &args).await?;
            text.set(String::new());
            pending_images.set(Vec::new());
            pick_mode.set(false);
            apply_turn(res);
            Ok(())
        }));
    };
    let send = move || send_text(text.get_untracked());
    let say = move |said: String| {
        text.set(said.clone());
        send_text(said);
    };
    let attach = move || {
        let Some(id) = design_id() else { return };
        if pending_images.with_untracked(|l| l.len() >= MAX_IMAGES) {
            return;
        }
        // 还没有模型时贴的图就是参考图;有了模型之后贴的图随那句话走(后端会让视觉模型先描述它)
        let as_reference = !has_model.get_untracked();
        spawn_local(async move {
            let Some(path) = ipc::pick_file("Image", &["png", "jpg", "jpeg", "webp"]).await else {
                return;
            };
            let args = serde_json::json!({ "id": id, "path": path, "as_reference": as_reference });
            match ipc::call::<_, Asset>(cmd::DESIGN_ADD_IMAGE, &args).await {
                Ok(asset) => pending_images.update(|l| l.push(asset)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let generate = move |spec: DesignSpec| {
        let Some(id) = design_id() else { return };
        // 第一版用「精细」:规格 → 完整脚本是最难的一步,值得多花几十秒
        let args = serde_json::json!({ "id": id, "spec": spec, "tier": CadTier::Precise });
        spec_dialog.set(false);
        pane.set(Pane::Chat);
        run_job(Box::pin(async move {
            apply_turn(ipc::call::<_, CadTurnResult>(cmd::DESIGN_GENERATE, &args).await?);
            Ok(())
        }));
    };
    let edit_spec = move |spec: DesignSpec| {
        spec_draft.set(Some(SpecDraft::from_spec(&spec)));
        spec_dialog.set(true);
    };
    let generate_from_dialog = move || {
        let Some(d) = spec_draft.get_untracked() else { return };
        match d.to_spec() {
            Ok(spec) => generate(spec),
            Err(problem) => {
                let l = current_locale();
                state.notify_error(match problem {
                    SpecProblem::BadSize => td_string!(l, cad.spec_bad_size).to_string(),
                    SpecProblem::NoFeatures => td_string!(l, cad.spec_no_features).to_string(),
                    SpecProblem::BadDim(line) => td_string!(l, cad.spec_bad_dim, line = line).to_string(),
                });
            }
        }
    };
    let select_version = move |version_id: String| {
        let Some(id) = design_id() else { return };
        if design.with_untracked(|d| d.as_ref().and_then(|d| d.current_version_id.as_deref()) == Some(version_id.as_str())) {
            return;
        }
        spawn_local(async move {
            let args = serde_json::json!({ "id": id, "version_id": version_id });
            match ipc::call::<_, CadDesign>(cmd::DESIGN_SELECT_VERSION, &args).await {
                Ok(d) => {
                    if let Some(v) = versions.with_untracked(|l| l.iter().find(|v| v.id == version_id).cloned()) {
                        code.set(v.code);
                    }
                    picked.set(None);
                    viewer3d::clear_pick(VIEWER_ID);
                    design.set(Some(d));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let load_code = move |failed: String| {
        code.set(failed);
        show_code.set(true);
    };
    let actions = ChatActions {
        send: Callback::new(move |()| send()),
        attach: Callback::new(move |()| attach()),
        generate: Callback::new(move |(spec,)| generate(spec)),
        edit_spec: Callback::new(move |(spec,)| edit_spec(spec)),
        select_version: Callback::new(move |(v,)| select_version(v)),
        say: Callback::new(move |(said,)| say(said)),
        load_code: Callback::new(move |(c,)| load_code(c)),
    };

    // ---- 不经过模型的修改 ----
    let set_param = move |name: String, value: f64| {
        let Some(id) = design_id() else { return };
        let args = serde_json::json!({ "id": id, "name": name, "value": value });
        run_job(Box::pin(async move {
            apply_turn(ipc::call::<_, CadTurnResult>(cmd::DESIGN_SET_PARAM, &args).await?);
            Ok(())
        }));
    };
    let run_code = move || {
        let Some(id) = design_id() else { return };
        let args = serde_json::json!({ "id": id, "code": code.get_untracked() });
        run_job(Box::pin(async move {
            apply_turn(ipc::call::<_, CadTurnResult>(cmd::DESIGN_RUN_CODE, &args).await?);
            Ok(())
        }));
    };
    let review = move || {
        let Some(id) = design_id() else { return };
        let renders = viewer3d::snapshot_views(VIEWER_ID, REVIEW_SHOT_PX);
        if renders.is_empty() {
            return;
        }
        let args = serde_json::json!({ "id": id, "renders": renders });
        pane.set(Pane::Chat);
        run_job(Box::pin(async move {
            apply_turn(ipc::call::<_, CadTurnResult>(cmd::DESIGN_REVIEW, &args).await?);
            Ok(())
        }));
    };

    // ---- 设计的管理 ----
    let rename = move || {
        let (Some(id), Some(name)) = (design_id(), renaming.get_untracked()) else { return };
        renaming.set(None);
        if name.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            match ipc::call::<_, CadDesign>(cmd::DESIGN_RENAME, &serde_json::json!({ "id": id, "name": name })).await {
                Ok(d) => {
                    design.set(Some(d));
                    reload_list();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let link_project = move |project_id: Option<String>| {
        let Some(id) = design_id() else { return };
        spawn_local(async move {
            match ipc::call::<_, CadDesign>(cmd::DESIGN_LINK_PROJECT, &serde_json::json!({ "id": id, "project_id": project_id })).await {
                Ok(d) => design.set(Some(d)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let delete_design = move || {
        let Some(id) = design_id() else { return };
        delete_dialog.set(false);
        spawn_local(async move {
            match ipc::call_unit(cmd::DESIGN_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    design.set(None);
                    messages.set(Vec::new());
                    versions.set(Vec::new());
                    code.set(String::new());
                    designs.update(|l| l.retain(|d| d.id != id));
                    if let Some(next) = designs.with_untracked(|l| l.first().map(|d| d.id.clone())) {
                        open(next);
                    }
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let remove_reference = move |asset_id: String| {
        let Some(id) = design_id() else { return };
        spawn_local(async move {
            match ipc::call::<_, CadDesign>(cmd::DESIGN_REMOVE_REFERENCE, &serde_json::json!({ "id": id, "asset_id": asset_id })).await {
                Ok(d) => design.set(Some(d)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let export = move |format: &'static str| {
        let Some(version_id) = current.with_untracked(|v| v.as_ref().map(|v| v.id.clone())) else { return };
        let base = design.with_untracked(|d| d.as_ref().map(|d| d.name.clone())).unwrap_or_else(|| "model".into());
        spawn_local(async move {
            let Some(dest) = ipc::pick_save_path("Export", &format!("{base}.{format}"), format).await else {
                return;
            };
            let args = serde_json::json!({ "version_id": version_id, "format": format, "dest_path": dest });
            match ipc::call_unit(cmd::CAD_EXPORT, &args).await {
                Ok(()) => state.notify_info(td_string!(current_locale(), lab.exported)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let open_external = move || {
        let Some(asset_id) = current.with_untracked(|v| v.as_ref().map(|v| v.stl_asset_id.clone())) else { return };
        spawn_local(async move {
            if let Err(e) = ipc::call_unit(cmd::OPEN_ASSET_EXTERNAL, &serde_json::json!({ "id": asset_id })).await {
                state.notify_error(e);
            }
        });
    };

    // ---- 3D 视图 ----
    Effect::new(move |prev: Option<()>| {
        if prev.is_some() {
            return;
        }
        let dark = state.theme.get_untracked() == Theme::Dark;
        spawn_local(async move {
            match viewer3d::mount(VIEWER_ID, dark).await {
                Ok(()) => viewer_ready.set(true),
                Err(e) => {
                    crate::main_log("studio", &format!("viewer mount failed: {e}"));
                    viewer_error.set(Some(e));
                }
            }
        });
    });
    on_cleanup(move || {
        pick_handler.set_value(None);
        viewer3d::dispose(VIEWER_ID);
    });
    Effect::new(move |_| {
        if !viewer_ready.get() {
            return;
        }
        let Some((id, parent, asset_id, design_id)) =
            current.with(|v| v.as_ref().map(|v| (v.id.clone(), v.parent_id.clone(), v.stl_asset_id.clone(), v.design_id.clone())))
        else {
            return;
        };
        if loaded_id.with_value(|prev| prev.as_deref() == Some(id.as_str())) {
            return;
        }
        // 子版本(改参数 / 对话修改)保持视角;换到别的模型才重新取景
        let keep_view = loaded_id.with_value(|prev| prev.is_some() && *prev == parent);
        loaded_id.set_value(Some(id.clone()));
        let is_newest = versions.with_untracked(|l| l.first().is_some_and(|v| v.id == id));
        spawn_local(async move {
            match viewer3d::load_model(VIEWER_ID, &ipc::asset_url(&asset_id), "stl", keep_view).await {
                Ok(_) => {
                    // 最新的版本顺手拍一张缩略图给列表用
                    if let (true, Some(design_id)) = (is_newest, design_id) {
                        if let Some(thumb) = viewer3d::snapshot_views(VIEWER_ID, THUMB_PX).into_iter().next() {
                            let args = serde_json::json!({ "id": design_id, "thumb": thumb });
                            if ipc::call_unit(cmd::DESIGN_SET_THUMB, &args).await.is_ok() {
                                designs.update(|l| {
                                    if let Some(row) = l.iter_mut().find(|d| d.id == design_id) {
                                        row.thumb = Some(thumb);
                                    }
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    crate::main_log("studio", &format!("load {asset_id} failed: {e}"));
                    state.notify_error(format!("{}: {e}", td_string!(current_locale(), lab.viewer_failed)));
                }
            }
        });
    });
    Effect::new(move |_| {
        if viewer_ready.get() {
            viewer3d::set_display(VIEWER_ID, false, show_bed.get());
            viewer3d::set_edges(VIEWER_ID, edges.get());
        }
    });
    Effect::new(move |_| {
        if viewer_ready.get() {
            viewer3d::set_dark(VIEWER_ID, state.theme.get() == Theme::Dark);
        }
    });
    Effect::new(move |_| {
        if !viewer_ready.get() {
            return;
        }
        if pick_mode.get() {
            pick_handler.set_value(Some(viewer3d::enable_pick(VIEWER_ID, move |p| picked.set(Some(p)))));
        } else {
            viewer3d::disable_pick(VIEWER_ID);
            pick_handler.set_value(None);
            picked.set(None);
        }
    });
    Effect::new(move |_| set_pref("studio_tier", tier.get().as_str()));

    let progress_text = Signal::derive(move || {
        let Some((step, attempt, problems)) = state.cad_progress.get() else {
            return t_string!(i18n, cad.working).to_string();
        };
        match step.as_str() {
            "reading_image" => t_string!(i18n, cad.step_reading_image).to_string(),
            "writing_code" => t_string!(i18n, cad.step_writing_code, attempt = attempt).to_string(),
            "running" => t_string!(i18n, cad.step_running, attempt = attempt).to_string(),
            "repairing" => t_string!(i18n, cad.step_repairing, attempt = attempt, problems = problems).to_string(),
            "reviewing" => t_string!(i18n, cad.step_reviewing).to_string(),
            _ => t_string!(i18n, cad.working).to_string(),
        }
    });
    // 还没有模型时只需要视觉 / 文本模型;有了模型之后每句话都可能要跑引擎
    let can_send = Signal::derive(move || {
        !busy.get()
            && design.with(Option::is_some)
            && (!text.with(|t| t.trim().is_empty()) || !pending_images.with(Vec::is_empty))
            && (!has_model.get() || engine_ok())
    });
    let can_run_code = Signal::derive(move || !busy.get() && engine_ok() && design.with(Option::is_some) && !code.with(|c| c.trim().is_empty()));
    let no_model = Signal::derive(move || busy.get() || !has_model.get());
    let can_review = Signal::derive(move || !busy.get() && has_model.get() && design.with(|d| d.as_ref().is_some_and(|d| !d.ref_asset_ids.is_empty())));
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center justify-between gap-4 border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, studio.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, studio.subtitle)}</p>
                </div>
            </header>

            <div class="flex-1 min-h-0 flex">
                // ---- 左:设计列表 ----
                <aside class="w-52 shrink-0 h-full flex flex-col border-r border-gray-200 dark:border-gray-700">
                    <div class="p-3 space-y-3 border-b border-gray-200 dark:border-gray-700">
                        <EngineBanner engine=engine installing=installing progress=state.cad_engine_progress on_recheck=check_engine/>
                        <Button icon=IconKind::Plus on_click=new_design>{move || t_string!(i18n, studio.new_design)}</Button>
                        <Show when=demo>
                            <p class="mt-2 text-[11px] leading-relaxed text-amber-600 dark:text-amber-400">{move || t_string!(i18n, studio.demo_note)}</p>
                        </Show>
                    </div>
                    <div class="flex-1 min-h-0 overflow-y-auto p-2 space-y-1">
                        <Show when=move || designs.with(Vec::is_empty)>
                            <p class="p-3 text-xs leading-relaxed text-gray-400">{move || t_string!(i18n, studio.no_designs)}</p>
                        </Show>
                        <For each=move || designs.get() key=|d| (d.id.clone(), d.updated_at, d.thumb.is_some(), d.name.clone()) let:row>
                            <DesignRow row=row design=design on_open=open/>
                        </For>
                    </div>
                </aside>

                // 工作区一直渲染着(3D 视图的容器要在挂载时就存在);还没打开设计时盖一层空状态
                <div class="relative flex-1 min-w-0 h-full flex">
                    <Show when=move || design.with(Option::is_none)>
                        <div class="absolute inset-0 z-10 bg-gray-50 dark:bg-gray-900">
                            <EmptyState icon=IconKind::Box title=move || t_string!(i18n, studio.empty_title) hint=move || t_string!(i18n, studio.empty_hint)>
                                <Button icon=IconKind::Plus on_click=new_design>{move || t_string!(i18n, studio.new_design)}</Button>
                            </EmptyState>
                        </div>
                    </Show>
                    // ---- 中:3D 视图 + 代码 ----
                    <section class="flex-1 min-w-0 h-full flex flex-col">
                        <div class="shrink-0 h-11 px-3 flex items-center gap-3 border-b border-gray-200 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap overflow-hidden">
                            {move || match renaming.get() {
                                Some(_) => view! {
                                    <input
                                        type="text"
                                        class="w-48 h-7 px-2 rounded-md border border-brand bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100 focus:outline-none"
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
                                        class="min-w-0 max-w-[12rem] truncate text-sm font-semibold text-gray-900 dark:text-gray-50 hover:text-brand"
                                        title=move || t_string!(i18n, studio.rename)
                                        on:click=move |_| renaming.set(design.with_untracked(|d| d.as_ref().map(|d| d.name.clone())))
                                    >
                                        {move || design.with(|d| d.as_ref().map(|d| d.name.clone()).unwrap_or_default())}
                                    </button>
                                }.into_any(),
                            }}
                            <select
                                class="h-7 min-w-0 w-32 shrink px-1.5 rounded-md border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-900 text-xs text-gray-600 dark:text-gray-300"
                                title=move || t_string!(i18n, studio.link_project)
                                on:change=move |e| {
                                    let v = event_target_value(&e);
                                    link_project((!v.is_empty()).then_some(v));
                                }
                            >
                                <option value="" selected=move || design.with(|d| d.as_ref().is_some_and(|d| d.project_id.is_none()))>
                                    {move || t_string!(i18n, studio.no_project)}
                                </option>
                                {move || state.projects.get().into_iter().map(|p| {
                                    let pid = p.id.clone();
                                    view! {
                                        <option
                                            value=p.id.clone()
                                            selected=move || design.with(|d| d.as_ref().and_then(|d| d.project_id.as_deref()) == Some(pid.as_str()))
                                        >{format!("{} {}", p.code, p.title)}</option>
                                    }
                                }).collect_view()}
                            </select>
                            <div class="flex-1"></div>
                            <label class="shrink-0 flex items-center gap-1.5">
                                <Toggle checked=edges on_change=move |v: bool| edges.set(v)/>
                                {move || t_string!(i18n, cad.edges)}
                            </label>
                            <label class="shrink-0 flex items-center gap-1.5">
                                <Toggle checked=show_bed on_change=move |v: bool| show_bed.set(v)/>
                                {move || t_string!(i18n, lab.show_bed)}
                            </label>
                            <label class="shrink-0 flex items-center gap-1.5">
                                <Toggle checked=show_code on_change=move |v: bool| show_code.set(v)/>
                                <Icon kind=IconKind::Code class="w-3.5 h-3.5"/>
                                {move || t_string!(i18n, cad.show_code)}
                            </label>
                            <IconButton icon=IconKind::Trash label=move || t_string!(i18n, studio.delete_design) on_click=move || delete_dialog.set(true)/>
                        </div>
                        <div class="relative flex-1 min-h-0">
                            <div id=VIEWER_ID class="absolute inset-0"></div>
                            <Show when=move || !viewer_ready.get()>
                                <div class="absolute inset-0 flex items-center justify-center text-xs text-gray-400">
                                    {move || match viewer_error.get() {
                                        Some(e) => format!("{}: {e}", t_string!(i18n, lab.viewer_failed)),
                                        None => t_string!(i18n, lab.viewer_loading).to_string(),
                                    }}
                                </div>
                            </Show>
                            <Show when=move || viewer_ready.get() && !has_model.get()>
                                <div class="absolute inset-0 pointer-events-none">
                                    <EmptyState icon=IconKind::Box title=move || t_string!(i18n, cad.viewer_empty) hint=move || t_string!(i18n, studio.viewer_empty_hint)/>
                                </div>
                            </Show>
                            // 左上角:当前版本的尺寸;右下角:导出
                            <Show when=move || has_model.get()>
                                <div class="absolute left-3 top-3 px-2 py-1 rounded-md bg-white/85 dark:bg-gray-900/80 text-[11px] tabular-nums text-gray-600 dark:text-gray-300 selectable">
                                    {move || current.with(|v| v.as_ref().map(|v| {
                                        let s = v.metrics.size;
                                        format!("{} × {} × {} mm · {:.1} cm³", fmt_mm(s[0]), fmt_mm(s[1]), fmt_mm(s[2]), v.metrics.volume_mm3 / 1000.0)
                                    }).unwrap_or_default())}
                                </div>
                            </Show>
                            <div class="absolute right-3 bottom-3 flex items-center gap-1.5">
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_model on_click=move || export("step")>"STEP"</Button>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_model on_click=move || export("stl")>"STL"</Button>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_model on_click=move || export("3mf")>"3MF"</Button>
                                <Button small=true icon=IconKind::External disabled=no_model on_click=open_external>{move || t_string!(i18n, studio.open_slicer)}</Button>
                            </div>
                        </div>
                        <Show when=move || show_code.get()>
                            <div class="shrink-0 h-72 flex flex-col border-t border-gray-200 dark:border-gray-700">
                                <textarea
                                    class="flex-1 min-h-0 w-full p-3 resize-none bg-gray-50 dark:bg-gray-900 text-[12px] leading-5 font-mono \
                                           text-gray-800 dark:text-gray-100 focus:outline-none selectable"
                                    spellcheck="false"
                                    autocomplete="off"
                                    wrap="off"
                                    placeholder=move || t_string!(i18n, cad.code_placeholder)
                                    prop:value=move || code.get()
                                    on:input=move |e| code.set(event_target_value(&e))
                                ></textarea>
                                <div class="shrink-0 px-3 py-2 flex justify-end border-t border-gray-200 dark:border-gray-700">
                                    <Button small=true icon=IconKind::Play disabled=Signal::derive(move || !can_run_code.get()) on_click=run_code>
                                        {move || t_string!(i18n, cad.run_code)}
                                    </Button>
                                </div>
                            </div>
                        </Show>
                    </section>

                    // ---- 右:对话 · 参数 · 版本 ----
                    <aside class="w-[24rem] shrink-0 h-full flex flex-col border-l border-gray-200 dark:border-gray-700">
                        <div class="shrink-0 h-11 px-3 flex items-center justify-between gap-2 border-b border-gray-200 dark:border-gray-700">
                            <Segmented
                                value=Signal::derive(move || pane.get())
                                options=vec![
                                    (Pane::Chat, label(|l| td_string!(l, studio.pane_chat))),
                                    (Pane::Params, label(|l| td_string!(l, studio.pane_params))),
                                    (Pane::Versions, label(|l| td_string!(l, studio.pane_versions))),
                                ]
                                on_change=move |p: Pane| pane.set(p)
                            />
                            <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Eye disabled=Signal::derive(move || !can_review.get()) on_click=review>
                                {move || t_string!(i18n, cad.review_title)}
                            </Button>
                        </div>
                        // 对话栏一直挂着(切到别的页签时只是藏起来):输入到一半的话、滚动位置都不丢
                        <div class="flex-1 min-h-0" class=("hidden", move || pane.get() != Pane::Chat)>
                            <ChatPane
                                messages=messages
                                design=design
                                busy=busy
                                progress_text=progress_text
                                can_send=can_send
                                text=text
                                pending_images=pending_images
                                tier=tier
                                pick_mode=pick_mode
                                picked=picked
                                actions=actions
                            />
                        </div>
                        <Show when=move || pane.get() == Pane::Params>
                            <div class="flex-1 min-h-0 overflow-y-auto p-3 space-y-3">
                                {move || match current.get() {
                                    None => view! { <p class="p-3 text-xs text-gray-400">{move || t_string!(i18n, studio.params_empty)}</p> }.into_any(),
                                    Some(v) => {
                                        let report = v.report.clone();
                                        let params = v.params.clone();
                                        view! {
                                            <ParamsCard params=params busy=busy on_change=set_param/>
                                            <MetricsCard version=v/>
                                            {report.map(|r| view! { <ReportCard report=r/> })}
                                        }.into_any()
                                    }
                                }}
                            </div>
                        </Show>
                        <Show when=move || pane.get() == Pane::Versions>
                            <div class="flex-1 min-h-0 overflow-y-auto p-3 space-y-3">
                                <RefStrip design=design on_remove=remove_reference/>
                                <Show when=move || versions.with(Vec::is_empty)>
                                    <p class="p-3 text-xs text-gray-400">{move || t_string!(i18n, studio.versions_empty)}</p>
                                </Show>
                                <div class="space-y-1">
                                    <For each=move || versions.get() key=|v| v.id.clone() let:version>
                                        <VersionRow version=version current=current on_select=select_version/>
                                    </For>
                                </div>
                            </div>
                        </Show>
                    </aside>
                </div>
            </div>

            // ---- 编辑规格 ----
            <Dialog open=spec_dialog title=move || t_string!(i18n, cad.spec_title) wide=true>
                <div class="max-h-[65vh] overflow-y-auto pr-1">
                    <p class="mb-3 text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || t_string!(i18n, cad.spec_hint)}</p>
                    {move || spec_draft.get().map(|d| view! { <SpecEditor draft=d build_mm=BUILD_VOLUME/> })}
                </div>
                <DialogFooter>
                    <Button variant=ButtonVariant::Secondary on_click=move || spec_dialog.set(false)>{move || t_string!(i18n, common.cancel)}</Button>
                    <Button icon=IconKind::Sparkles disabled=Signal::derive(move || busy.get() || !engine_ok()) on_click=generate_from_dialog>
                        {move || t_string!(i18n, studio.generate)}
                    </Button>
                </DialogFooter>
            </Dialog>

            // ---- 删除设计 ----
            <Dialog open=delete_dialog title=move || t_string!(i18n, studio.delete_design)>
                <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">{move || t_string!(i18n, studio.delete_confirm)}</p>
                <DialogFooter>
                    <Button variant=ButtonVariant::Secondary on_click=move || delete_dialog.set(false)>{move || t_string!(i18n, common.cancel)}</Button>
                    <Button variant=ButtonVariant::Danger icon=IconKind::Trash on_click=delete_design>{move || t_string!(i18n, common.delete)}</Button>
                </DialogFooter>
            </Dialog>
        </div>
    }
}

#[component]
fn DesignRow(row: CadDesignSummary, design: RwSignal<Option<CadDesign>>, #[prop(into)] on_open: Callback<(String,)>) -> impl IntoView {
    let id = StoredValue::new(row.id.clone());
    let active = move || design.with(|d| d.as_ref().is_some_and(|d| id.with_value(|id| &d.id == id)));
    let size = row.size.map(|s| format!("{} × {} × {}", fmt_mm(s[0]), fmt_mm(s[1]), fmt_mm(s[2])));
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
                {match row.thumb.clone() {
                    Some(src) => view! { <img src=src class="w-full h-full object-cover" draggable="false"/> }.into_any(),
                    None => view! { <Icon kind=IconKind::Box class="w-5 h-5"/> }.into_any(),
                }}
            </div>
            <div class="min-w-0 flex-1">
                <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{row.name.clone()}</div>
                <div class="text-[11px] tabular-nums text-gray-400 truncate">
                    {size.unwrap_or_else(|| "—".to_string())}
                    {(row.versions > 0).then(|| format!(" · v{}", row.versions))}
                </div>
            </div>
        </div>
    }
}

/// 这个设计的参考图(看图出规格、看图复核都对照它们)。
#[component]
fn RefStrip(design: RwSignal<Option<CadDesign>>, #[prop(into)] on_remove: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let refs = move || design.with(|d| d.as_ref().map(|d| d.ref_asset_ids.clone()).unwrap_or_default());
    view! {
        <Show when=move || !refs().is_empty()>
            <Card class="p-3 space-y-2">
                <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, cad.refs)}</div>
                <div class="flex flex-wrap gap-2">
                    <For each=refs key=|id| id.clone() let:asset_id>
                        {
                            let id = StoredValue::new(asset_id.clone());
                            view! {
                                <div class="group relative w-14 h-14 rounded-lg overflow-hidden border border-gray-200 dark:border-gray-700">
                                    <img src=ipc::asset_url(&asset_id) class="w-full h-full object-cover" draggable="false"/>
                                    <div class="absolute top-0 right-0 opacity-0 group-hover:opacity-100 transition-opacity rounded bg-white/90 dark:bg-gray-800/90">
                                        <IconButton icon=IconKind::Close label=move || t_string!(i18n, common.delete) on_click=move || on_remove.run((id.get_value(),))/>
                                    </div>
                                </div>
                            }
                        }
                    </For>
                </div>
            </Card>
        </Show>
    }
}

#[component]
fn VersionRow(version: CadVersion, current: Memo<Option<CadVersion>>, #[prop(into)] on_select: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(version.id.clone());
    let active = move || current.with(|c| c.as_ref().is_some_and(|c| id.with_value(|id| &c.id == id)));
    let source = version.source.clone();
    let tone = source_tone(&source);
    let size = version.metrics.size;
    let changed = version.report.as_ref().map(|r| changed_text(&r.changed_sections)).unwrap_or_default();
    let warned = version.report.as_ref().is_some_and(|r| !r.warnings.is_empty());
    let detail = if version.note.is_empty() { changed } else { version.note.clone() };
    view! {
        <div
            class="px-2.5 py-2 rounded-lg cursor-pointer transition-colors space-y-0.5"
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| on_select.run((id.get_value(),))
        >
            <div class="flex items-center gap-1.5">
                <Badge tone=tone>{move || source_name(i18n.get_locale(), &source)}</Badge>
                <span class="text-[11px] tabular-nums text-gray-500 dark:text-gray-400 truncate">
                    {format!("{} × {} × {}", fmt_mm(size[0]), fmt_mm(size[1]), fmt_mm(size[2]))}
                </span>
                {warned.then(|| view! { <Icon kind=IconKind::Alert class="w-3.5 h-3.5 shrink-0 text-amber-500"/> })}
            </div>
            {(!detail.is_empty()).then(|| view! {
                <div class="text-xs truncate text-gray-700 dark:text-gray-200" title=detail.clone()>{detail.clone()}</div>
            })}
        </div>
    }
}

/// 引擎状态:检查中 / 就绪(一行小字)/ 没装(说明 + 重新检查)。
#[component]
fn EngineBanner(
    engine: RwSignal<Option<CadEngineInfo>>,
    installing: RwSignal<bool>,
    progress: RwSignal<Option<(String, u64, u64)>>,
    #[prop(into)] on_recheck: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    move || match engine.get() {
        _ if installing.get() => {
            let mb = engine.with(|e| e.as_ref().map(|e| e.unpacked_bytes / 1_000_000).unwrap_or(0));
            // 校验占前 15%,解包占后 85%(解包慢得多)
            let percent = move || match progress.get() {
                Some((phase, done, total)) if total > 0 => {
                    let part = done as f64 / total as f64;
                    if phase == "verifying" {
                        part * 15.0
                    } else {
                        15.0 + part * 85.0
                    }
                }
                _ => 0.0,
            };
            view! {
                <div class="rounded-lg border border-indigo-200 bg-indigo-50 dark:bg-indigo-500/10 dark:border-indigo-500/40 px-3 py-2.5 space-y-2">
                    <div class="text-xs font-semibold text-indigo-900 dark:text-indigo-100">{move || t_string!(i18n, cad.engine_installing_title)}</div>
                    <p class="text-xs leading-relaxed text-indigo-900/80 dark:text-indigo-100/80">
                        {move || t_string!(i18n, cad.engine_installing_hint, mb = mb).to_string()}
                    </p>
                    <div class="h-1.5 rounded-full bg-indigo-200 dark:bg-indigo-900 overflow-hidden">
                        <div class="h-full bg-brand transition-all" style:width=move || format!("{:.1}%", percent())></div>
                    </div>
                </div>
            }
            .into_any()
        }
        None => view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, cad.engine_checking)}</p> }.into_any(),
        Some(info) if info.available => view! {
            <p class="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400">
                <span class="w-1.5 h-1.5 rounded-full bg-green-500"></span>
                {move || t_string!(
                    i18n,
                    cad.engine_ready,
                    version = info.build123d_version.clone(),
                    python = info.python_version.clone(),
                ).to_string()}
            </p>
        }
        .into_any(),
        Some(info) => view! {
            <div class="rounded-lg border border-amber-300 bg-amber-50 dark:bg-amber-500/10 dark:border-amber-500/40 px-3 py-2.5 space-y-2">
                <div class="text-xs font-semibold text-amber-800 dark:text-amber-200">{move || t_string!(i18n, cad.engine_missing_title)}</div>
                // 正式版(带引擎包)出问题 = 解包失败;开发构建 = 还没装开发用的引擎
                <p class="text-xs leading-relaxed text-amber-800/90 dark:text-amber-200/90 selectable">
                    {move || if info.bundled {
                        t_string!(i18n, cad.engine_unpack_failed_hint)
                    } else {
                        t_string!(i18n, cad.engine_missing_hint)
                    }}
                </p>
                {(!info.problem.is_empty()).then(|| view! {
                    <p class="text-[11px] font-mono break-all text-amber-700/80 dark:text-amber-300/80 selectable">{info.problem.clone()}</p>
                })}
                <Button small=true variant=ButtonVariant::Secondary on_click=move || on_recheck.run(())>
                    {move || t_string!(i18n, cad.recheck)}
                </Button>
            </div>
        }
        .into_any(),
    }
}
