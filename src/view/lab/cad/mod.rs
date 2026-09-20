//! 预研页 · 代码建模面板(docs/adr/0003-code-cad-build123d.md)。
//!
//!   左:参考图 + 要求 → 设计规格(给人审)→ 生成;下面是版本列表
//!   中:3D 视图(棱线、点选位置)+ 代码框
//!   右:指标 · 过程报告 · 参数面板(不经过模型)· 指令修改(局部)· 看图复核 · 导出
//!
//! 三种改法的代价从低到高:改参数(本机重建,一两秒,不花钱)→ 指令修改(只动相关的段)→ 改规格重新生成。

mod side;
mod spec_editor;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::cad::{CadBuildReport, CadBuildResult, CadEngineInfo, CadPick, CadReview, CadReviewResult, CadSpecResult, CadVersion};
use pp_common::Asset;

use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::current_locale;
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::{Badge, Button, ButtonVariant, Card, EmptyState, Field, IconButton, SectionTitle, TextArea, Toggle, Tone};
use crate::utils::fmt_mm;
use crate::viewer3d::{self, Pick, PickHandler};
use side::{changed_text, MetricsCard, ParamsCard, ReportCard};
use spec_editor::{SpecDraft, SpecEditor, SpecProblem};

const VIEWER_ID: &str = "cad-viewer";
pub(super) const BUILD_VOLUME: [f64; 3] = [256.0, 256.0, 256.0];
const MAX_REFS: usize = 4;
/// 复核截图的边长(像素)。视觉模型每张图的 token 有上限,再大也是白传
const REVIEW_SHOT_PX: u32 = 768;

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
pub fn CadPanel(state: AppState) -> impl IntoView {
    let i18n = use_i18n();

    // ---- 状态 ----
    let engine = RwSignal::new(None::<CadEngineInfo>); // None = 还在检查
    let refs = RwSignal::new(Vec::<Asset>::new());
    let brief = RwSignal::new(String::new());
    let draft = RwSignal::new(None::<SpecDraft>);
    let versions = RwSignal::new(Vec::<CadVersion>::new());
    let selected = RwSignal::new(None::<CadVersion>);
    let busy = RwSignal::new(false);
    // 整个没建成的那一次:报告留着给用户看(代码已经放进代码框)
    let failed = RwSignal::new(None::<CadBuildReport>);

    let code = RwSignal::new(String::new());
    let show_code = RwSignal::new(false);
    let instruction = RwSignal::new(String::new());
    let review = RwSignal::new(None::<CadReview>);

    let viewer_ready = RwSignal::new(false);
    let viewer_error = RwSignal::new(None::<String>);
    let edges = RwSignal::new(true);
    let show_bed = RwSignal::new(true);
    let pick_mode = RwSignal::new(false);
    let picked = RwSignal::new(None::<Pick>);
    let pick_handler = StoredValue::new_local(None::<PickHandler>);
    // 上一次加载进视图的版本:新版本如果是它的子版本,就保持视角不动
    let loaded_id = StoredValue::new(None::<String>);

    let demo = move || state.app_info.with(|i| i.as_ref().is_some_and(|i| i.demo_mode));
    let engine_ok = move || engine.with(|e| e.as_ref().is_some_and(|e| e.available));
    let idle = Signal::derive(move || !busy.get());

    // ---- 数据 ----
    // 引擎状态。安装包里带着引擎包、但还没解开(首次使用 / 升级后引擎版本变了)→ 直接开始解,不用用户点
    let installing = RwSignal::new(false);
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
                    // 解包失败:留在「没装好」的状态,把原因摆出来;「重新检查」会再试一次
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

    let select = move |v: CadVersion| {
        code.set(v.code.clone());
        review.set(None);
        failed.set(None);
        picked.set(None);
        viewer3d::clear_pick(VIEWER_ID);
        selected.set(Some(v));
    };

    spawn_local(async move {
        match ipc::call::<_, Vec<CadVersion>>(cmd::CAD_LIST_VERSIONS, &serde_json::json!({ "project_id": null })).await {
            Ok(list) => {
                if let Some(first) = list.first().cloned() {
                    select(first);
                }
                versions.set(list);
            }
            Err(e) => state.notify_error(e),
        }
    });

    // 一个新版本(生成 / 改参数 / 修补 / 手写运行):进列表并选中
    let adopt = move |v: CadVersion| {
        state.notify_info(td_string!(current_locale(), lab.done_in, ms = v.elapsed_ms).to_string());
        versions.update(|list| list.insert(0, v.clone()));
        select(v);
    };
    // 生成 / 修补的结果:建成了就采用;一版都没建成,把最后的代码和每一轮的报错摆出来
    let settle = move |res: CadBuildResult| match res.version {
        Some(v) => {
            adopt(v);
            instruction.set(String::new());
            pick_mode.set(false);
        }
        None => {
            code.set(res.code);
            show_code.set(true);
            failed.set(Some(res.report));
        }
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

    let add_ref = move || {
        spawn_local(async move {
            let Some(path) = ipc::pick_file("Image", &["png", "jpg", "jpeg", "webp"]).await else {
                return;
            };
            let args = serde_json::json!({ "path": path, "project_id": null });
            match ipc::call::<_, Asset>(cmd::CAD_IMPORT_REFERENCE, &args).await {
                Ok(asset) => refs.update(|list| list.push(asset)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let ref_ids = move || refs.with_untracked(|list| list.iter().map(|a| a.id.clone()).collect::<Vec<_>>());

    let make_spec = move || {
        let args = serde_json::json!({ "brief": brief.get_untracked(), "ref_asset_ids": ref_ids() });
        run_job(Box::pin(async move {
            let res = ipc::call::<_, CadSpecResult>(cmd::CAD_DESIGN_SPEC, &args).await?;
            draft.set(Some(SpecDraft::from_spec(&res.spec)));
            Ok(())
        }));
    };

    let generate = move || {
        let Some(d) = draft.get_untracked() else { return };
        let spec = match d.to_spec() {
            Ok(spec) => spec,
            Err(problem) => {
                let l = current_locale();
                state.notify_error(match problem {
                    SpecProblem::BadSize => td_string!(l, cad.spec_bad_size).to_string(),
                    SpecProblem::NoFeatures => td_string!(l, cad.spec_no_features).to_string(),
                    SpecProblem::BadDim(line) => td_string!(l, cad.spec_bad_dim, line = line).to_string(),
                });
                return;
            }
        };
        let args = serde_json::json!({ "spec": spec, "ref_asset_ids": ref_ids(), "project_id": null });
        run_job(Box::pin(async move {
            settle(ipc::call::<_, CadBuildResult>(cmd::CAD_GENERATE, &args).await?);
            Ok(())
        }));
    };

    let selected_id = move || selected.with_untracked(|v| v.as_ref().map(|v| v.id.clone()));

    let set_param = move |name: String, value: f64| {
        let Some(id) = selected_id() else { return };
        let args = serde_json::json!({ "version_id": id, "name": name, "value": value });
        run_job(Box::pin(async move {
            adopt(ipc::call::<_, CadVersion>(cmd::CAD_SET_PARAM, &args).await?);
            Ok(())
        }));
    };

    let edit = move |text: String| {
        let Some(id) = selected_id() else { return };
        if text.trim().is_empty() {
            return;
        }
        let pick = picked.get_untracked().map(|p| CadPick {
            point: p.point,
            normal: Some(p.normal),
        });
        let args = serde_json::json!({ "version_id": id, "instruction": text, "pick": pick });
        run_job(Box::pin(async move {
            settle(ipc::call::<_, CadBuildResult>(cmd::CAD_EDIT, &args).await?);
            Ok(())
        }));
    };

    let run_code = move || {
        let args = serde_json::json!({ "code": code.get_untracked(), "project_id": null, "parent_id": selected_id() });
        run_job(Box::pin(async move {
            adopt(ipc::call::<_, CadVersion>(cmd::CAD_RUN, &args).await?);
            Ok(())
        }));
    };

    let run_review = move || {
        let Some(id) = selected_id() else { return };
        let renders = viewer3d::snapshot_views(VIEWER_ID, REVIEW_SHOT_PX);
        if renders.is_empty() {
            return;
        }
        let args = serde_json::json!({ "version_id": id, "renders": renders });
        run_job(Box::pin(async move {
            let res = ipc::call::<_, CadReviewResult>(cmd::CAD_REVIEW, &args).await?;
            review.set(Some(res.review));
            Ok(())
        }));
    };

    let export = move |format: &'static str| {
        let Some(id) = selected_id() else { return };
        spawn_local(async move {
            let Some(dest) = ipc::pick_save_path("Export", &format!("model.{format}"), format).await else {
                return;
            };
            let args = serde_json::json!({ "version_id": id, "format": format, "dest_path": dest });
            match ipc::call_unit(cmd::CAD_EXPORT, &args).await {
                Ok(()) => state.notify_info(td_string!(current_locale(), lab.exported)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let open_external = move || {
        let Some(asset_id) = selected.with_untracked(|v| v.as_ref().map(|v| v.stl_asset_id.clone())) else {
            return;
        };
        spawn_local(async move {
            if let Err(e) = ipc::call_unit(cmd::OPEN_ASSET_EXTERNAL, &serde_json::json!({ "id": asset_id })).await {
                state.notify_error(e);
            }
        });
    };
    let remove = move |id: String| {
        spawn_local(async move {
            match ipc::call_unit(cmd::CAD_DELETE_VERSION, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    versions.update(|list| list.retain(|v| v.id != id));
                    if selected.with_untracked(|s| s.as_ref().is_some_and(|v| v.id == id)) {
                        match versions.with_untracked(|l| l.first().cloned()) {
                            Some(next) => select(next),
                            None => selected.set(None),
                        }
                    }
                }
                Err(e) => state.notify_error(e),
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
                    crate::main_log("cad", &format!("viewer mount failed: {e}"));
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
        let Some((id, parent, asset_id)) = selected.with(|v| v.as_ref().map(|v| (v.id.clone(), v.parent_id.clone(), v.stl_asset_id.clone())))
        else {
            return;
        };
        // 子版本(改参数 / 修补)保持视角;换到别的模型才重新取景
        let keep_view = loaded_id.with_value(|prev| prev.is_some() && *prev == parent);
        loaded_id.set_value(Some(id));
        spawn_local(async move {
            if let Err(e) = viewer3d::load_model(VIEWER_ID, &ipc::asset_url(&asset_id), "stl", keep_view).await {
                crate::main_log("cad", &format!("load {asset_id} failed: {e}"));
                state.notify_error(format!("{}: {e}", td_string!(current_locale(), lab.viewer_failed)));
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

    let progress_text = move || {
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
    };

    let can_spec = Signal::derive(move || !busy.get() && (!refs.with(Vec::is_empty) || !brief.with(|b| b.trim().is_empty())));
    let can_generate = Signal::derive(move || !busy.get() && engine_ok() && draft.with(Option::is_some));
    let can_run_code = Signal::derive(move || !busy.get() && engine_ok() && !code.with(|c| c.trim().is_empty()));
    let can_edit = Signal::derive(move || !busy.get() && selected.with(Option::is_some) && !instruction.with(|t| t.trim().is_empty()));
    let refs_full = Signal::derive(move || busy.get() || refs.with(|r| r.len() >= MAX_REFS));
    let no_selection = Signal::derive(move || busy.get() || selected.with(Option::is_none));

    view! {
        <div class="h-full flex">
            // ---- 左:输入、规格、版本 ----
            <aside class="w-80 shrink-0 h-full overflow-y-auto border-r border-gray-200 dark:border-gray-700 p-4 space-y-4">
                <EngineBanner engine=engine installing=installing progress=state.cad_engine_progress on_recheck=check_engine/>
                <Show when=demo>
                    <p class="text-xs leading-relaxed text-amber-600 dark:text-amber-400">{move || t_string!(i18n, cad.demo_note)}</p>
                </Show>

                <div class="space-y-2">
                    <SectionTitle title=move || t_string!(i18n, cad.refs) hint=move || t_string!(i18n, cad.refs_hint)/>
                    <div class="flex flex-wrap gap-2">
                        <For each=move || refs.get() key=|a| a.id.clone() let:asset>
                            <RefThumb asset=asset refs=refs/>
                        </For>
                        <button
                            type="button"
                            class="w-16 h-16 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 flex items-center justify-center \
                                   text-gray-400 hover:text-brand hover:border-brand transition-colors disabled:opacity-40 disabled:pointer-events-none"
                            disabled=move || refs_full.get()
                            title=move || t_string!(i18n, cad.add_ref)
                            on:click=move |_| add_ref()
                        >
                            <Icon kind=IconKind::Image class="w-5 h-5"/>
                        </button>
                    </div>
                </div>
                <Field label=move || t_string!(i18n, cad.brief)>
                    <TextArea value=brief rows=4 placeholder=move || t_string!(i18n, cad.brief_placeholder)/>
                </Field>
                <Button icon=IconKind::Eye variant=ButtonVariant::Secondary disabled=Signal::derive(move || !can_spec.get()) on_click=make_spec>
                    {move || t_string!(i18n, cad.make_spec)}
                </Button>

                {move || draft.get().map(|d| view! {
                    <Card class="p-3 space-y-3">
                        <SectionTitle title=move || t_string!(i18n, cad.spec_title) hint=move || t_string!(i18n, cad.spec_hint)/>
                        <SpecEditor draft=d build_mm=BUILD_VOLUME/>
                        <Button icon=IconKind::Sparkles disabled=Signal::derive(move || !can_generate.get()) on_click=generate>
                            {move || t_string!(i18n, cad.generate)}
                        </Button>
                    </Card>
                })}

                <div class="pt-3 border-t border-gray-200 dark:border-gray-700 space-y-2">
                    <SectionTitle title=move || t_string!(i18n, cad.versions)/>
                    <Show when=move || versions.with(Vec::is_empty)>
                        <p class="text-xs leading-relaxed text-gray-400">{move || t_string!(i18n, cad.no_versions)}</p>
                    </Show>
                    <div class="space-y-1">
                        <For each=move || versions.get() key=|v| v.id.clone() let:version>
                            <VersionRow version=version selected=selected on_select=select on_remove=remove/>
                        </For>
                    </div>
                </div>
            </aside>

            // ---- 中:3D 视图 + 代码 ----
            <section class="flex-1 min-w-0 h-full flex flex-col">
                <div class="shrink-0 h-10 px-3 flex items-center gap-4 border-b border-gray-200 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400">
                    <label class="flex items-center gap-1.5">
                        <Toggle checked=edges on_change=move |v: bool| edges.set(v)/>
                        {move || t_string!(i18n, cad.edges)}
                    </label>
                    <label class="flex items-center gap-1.5">
                        <Toggle checked=show_bed on_change=move |v: bool| show_bed.set(v)/>
                        {move || t_string!(i18n, lab.show_bed)}
                    </label>
                    <label class="flex items-center gap-1.5">
                        <Toggle checked=show_code on_change=move |v: bool| show_code.set(v)/>
                        <Icon kind=IconKind::Code class="w-3.5 h-3.5"/>
                        {move || t_string!(i18n, cad.show_code)}
                    </label>
                    <div class="flex-1"></div>
                    <Show when=move || busy.get()>
                        <Badge tone=Tone::Brand>{progress_text}</Badge>
                    </Show>
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
                    <Show when=move || viewer_ready.get() && selected.with(Option::is_none)>
                        <div class="absolute inset-0 pointer-events-none">
                            <EmptyState icon=IconKind::Box title=move || t_string!(i18n, cad.viewer_empty)/>
                        </div>
                    </Show>
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

            // ---- 右:指标、参数、修改、复核、导出 ----
            <aside class="w-80 shrink-0 h-full overflow-y-auto border-l border-gray-200 dark:border-gray-700 p-3 space-y-3">
                {move || failed.get().map(|report| view! { <ReportCard report=report failed=true/> })}
                {move || selected.get().map(|v| {
                    let has_refs = !v.ref_asset_ids.is_empty();
                    let report = v.report.clone();
                    let params = v.params.clone();
                    view! {
                        <MetricsCard version=v/>
                        {report.map(|r| view! { <ReportCard report=r/> })}
                        <ParamsCard params=params busy=busy on_change=set_param/>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, cad.edit_title) hint=move || t_string!(i18n, cad.edit_hint)/>
                            <TextArea value=instruction rows=3 placeholder=move || t_string!(i18n, cad.edit_placeholder)/>
                            <div class="flex items-center justify-between gap-2">
                                <label class="flex items-center gap-1.5 text-xs text-gray-500 dark:text-gray-400">
                                    <Toggle checked=pick_mode on_change=move |v: bool| pick_mode.set(v)/>
                                    <Icon kind=IconKind::Crosshair class="w-3.5 h-3.5"/>
                                    {move || match picked.get() {
                                        Some(p) => t_string!(
                                            i18n,
                                            cad.picked,
                                            x = fmt_mm(p.point[0]),
                                            y = fmt_mm(p.point[1]),
                                            z = fmt_mm(p.point[2]),
                                        ).to_string(),
                                        None => t_string!(i18n, cad.pick).to_string(),
                                    }}
                                </label>
                                <Button
                                    small=true
                                    icon=IconKind::Wand
                                    disabled=Signal::derive(move || !can_edit.get())
                                    on_click=move || edit(instruction.get_untracked())
                                >
                                    {move || t_string!(i18n, cad.edit_run)}
                                </Button>
                            </div>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, cad.review_title) hint=move || t_string!(i18n, cad.review_hint)/>
                            {if has_refs {
                                view! {
                                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Eye disabled=Signal::derive(move || !idle.get()) on_click=run_review>
                                        {move || t_string!(i18n, cad.review_run)}
                                    </Button>
                                }.into_any()
                            } else {
                                view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, cad.review_no_refs)}</p> }.into_any()
                            }}
                            {move || review.get().map(|r| view! { <ReviewResult review=r busy=busy on_fix=edit/> })}
                        </Card>
                    }
                })}

                <Card class="p-4 space-y-2">
                    <div class="flex flex-wrap gap-2">
                        <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_selection on_click=move || export("step")>
                            {move || t_string!(i18n, cad.export_step)}
                        </Button>
                        <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_selection on_click=move || export("stl")>
                            {move || t_string!(i18n, lab.export_stl)}
                        </Button>
                        <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Download disabled=no_selection on_click=move || export("3mf")>
                            {move || t_string!(i18n, lab.export_3mf)}
                        </Button>
                    </div>
                    <Button small=true icon=IconKind::External disabled=no_selection on_click=open_external>
                        {move || t_string!(i18n, lab.open_external)}
                    </Button>
                </Card>
            </aside>
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

#[component]
fn RefThumb(asset: Asset, refs: RwSignal<Vec<Asset>>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(asset.id.clone());
    view! {
        <div class="group relative w-16 h-16 rounded-lg overflow-hidden border border-gray-200 dark:border-gray-700 bg-gray-100 dark:bg-gray-900">
            <img src=ipc::asset_url(&asset.id) class="w-full h-full object-cover" draggable="false"/>
            <div class="absolute top-0.5 right-0.5 opacity-0 group-hover:opacity-100 transition-opacity rounded bg-white/90 dark:bg-gray-800/90">
                <IconButton
                    icon=IconKind::Close
                    label=move || t_string!(i18n, common.delete)
                    on_click=move || refs.update(|list| list.retain(|a| id.with_value(|id| &a.id != id)))
                />
            </div>
        </div>
    }
}

#[component]
fn VersionRow(
    version: CadVersion,
    selected: RwSignal<Option<CadVersion>>,
    #[prop(into)] on_select: Callback<(CadVersion,)>,
    #[prop(into)] on_remove: Callback<(String,)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(version.id.clone());
    let stored = StoredValue::new(version.clone());
    let active = move || selected.with(|s| s.as_ref().is_some_and(|v| id.with_value(|id| &v.id == id)));
    let source = version.source.clone();
    let tone = source_tone(&source);
    let size = version.metrics.size;
    let changed = version.report.as_ref().map(|r| changed_text(&r.changed_sections)).unwrap_or_default();
    let warned = version.report.as_ref().is_some_and(|r| !r.warnings.is_empty());
    // 第二行:这一版是怎么来的(指令原文 / 改了哪个参数 / 规格名),其次是改动的段
    let detail = if version.note.is_empty() { changed.clone() } else { version.note.clone() };

    view! {
        <div
            class="group flex items-center gap-2 px-2.5 py-2 rounded-lg cursor-pointer transition-colors"
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| on_select.run((stored.get_value(),))
        >
            <div class="flex-1 min-w-0 space-y-0.5">
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
            <div class="opacity-0 group-hover:opacity-100 transition-opacity" on:click=|e| e.stop_propagation()>
                <IconButton
                    icon=IconKind::Trash
                    label=move || t_string!(i18n, common.delete)
                    on_click=move || on_remove.run((id.get_value(),))
                />
            </div>
        </div>
    }
}

/// 复核结论。每条差异都是一句可以直接执行的修改指令——「按此修改」就是把它交给指令修改。
#[component]
fn ReviewResult(review: CadReview, busy: RwSignal<bool>, #[prop(into)] on_fix: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let n = review.differences.len();
    if review.matches && n == 0 {
        return view! {
            <p class="flex items-center gap-1.5 text-xs text-green-700 dark:text-green-300">
                <Icon kind=IconKind::Check class="w-3.5 h-3.5"/>
                {move || t_string!(i18n, cad.review_match)}
            </p>
        }
        .into_any();
    }
    view! {
        <div class="space-y-2">
            <div class="text-xs font-medium text-amber-700 dark:text-amber-300">
                {move || t_string!(i18n, cad.review_diff, n = n).to_string()}
            </div>
            {review
                .differences
                .into_iter()
                .map(|d| {
                    let text = StoredValue::new(d.clone());
                    view! {
                        <div class="rounded-lg border border-gray-200 dark:border-gray-700 p-2 space-y-1.5">
                            <p class="text-xs leading-relaxed text-gray-700 dark:text-gray-200 selectable">{d}</p>
                            <Button
                                small=true
                                variant=ButtonVariant::Ghost
                                icon=IconKind::Wand
                                disabled=Signal::derive(move || busy.get())
                                on_click=move || on_fix.run((text.get_value(),))
                            >
                                {move || t_string!(i18n, cad.review_fix)}
                            </Button>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
    .into_any()
}
