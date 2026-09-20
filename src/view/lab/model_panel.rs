//! 预研页 · 3D 模型面板:验证「three.js 视图桥」与「几何内核」。
//! 通过之后,这个面板的骨架就是 MVP-α 第 4 周的 3D 工作室(docs/02-ux-flows.md §3.5)。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::mesh::{MeshOpResult, MeshReport, PrintEstimateParams};
use pp_common::{Asset, AssetKind};

use crate::i18n_util::current_locale;
use crate::i18n::use_i18n;
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::{Badge, Button, ButtonVariant, Card, EmptyState, Field, IconButton, SectionTitle, Segmented, TextInput, Toggle, Tone};
use crate::utils::{fmt_int, fmt_mm, parse_positive};
use crate::viewer3d::{self, ViewerStats};

const VIEWER_ID: &str = "lab-viewer";
const BUILD_VOLUME: [f64; 3] = [256.0, 256.0, 256.0];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Sphere,
    Torus,
    Cuboid,
}

impl Shape {
    fn key(self) -> &'static str {
        match self {
            Shape::Sphere => "sphere",
            Shape::Torus => "torus",
            Shape::Cuboid => "box",
        }
    }
}

fn report_of(asset: &Asset) -> Option<MeshReport> {
    serde_json::from_str(asset.meta_json.as_deref()?).ok()
}

#[component]
pub fn ModelPanel(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let assets = RwSignal::new(Vec::<Asset>::new());
    let selected = RwSignal::new(None::<Asset>);
    let busy = RwSignal::new(false);

    let shape = RwSignal::new(Shape::Torus);
    let detail = RwSignal::new(3u32);
    let scale_mm = RwSignal::new("60".to_string());
    let cut_mm = RwSignal::new("2".to_string());

    let viewer_ready = RwSignal::new(false);
    let viewer_error = RwSignal::new(None::<String>);
    let stats = RwSignal::new(ViewerStats::default());
    let wireframe = RwSignal::new(false);
    let show_bed = RwSignal::new(true);
    let cut_preview = RwSignal::new(false);

    let report = Memo::new(move |_| selected.with(|a| a.as_ref().and_then(report_of)));

    // ---- 数据 ----
    let reload = move || {
        spawn_local(async move {
            let args = serde_json::json!({ "project_id": null, "kind": AssetKind::Model3d });
            match ipc::call::<_, Vec<Asset>>(cmd::LIST_ASSETS, &args).await {
                Ok(list) => {
                    if selected.with_untracked(Option::is_none) {
                        selected.set(list.first().cloned());
                    }
                    assets.set(list);
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    reload();

    // 所有网格操作走同一条路:调命令 → 新资产进列表并选中 → 报耗时
    let run_op = move |command: &'static str, args: serde_json::Value| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match ipc::call::<_, MeshOpResult>(command, &args).await {
                Ok(res) => {
                    state.notify_info(td_string!(current_locale(), lab.done_in, ms = res.elapsed_ms).to_string());
                    assets.update(|list| list.insert(0, res.asset.clone()));
                    selected.set(Some(res.asset));
                }
                Err(e) => state.notify_error(e),
            }
            busy.set(false);
        });
    };
    let selected_id = move || selected.with_untracked(|a| a.as_ref().map(|a| a.id.clone()));

    let generate = move || {
        run_op(
            cmd::LAB_GENERATE_SAMPLE,
            serde_json::json!({ "shape": shape.get_untracked().key(), "detail": detail.get_untracked() }),
        )
    };
    let import = move || {
        spawn_local(async move {
            if let Some(path) = ipc::pick_file("3D model", &["stl", "glb"]).await {
                run_op(cmd::IMPORT_MODEL, serde_json::json!({ "path": path, "project_id": null }));
            }
        });
    };
    let scale = move || {
        let (Some(id), Some(mm)) = (selected_id(), parse_positive(&scale_mm.get_untracked())) else {
            return;
        };
        run_op(cmd::MESH_SCALE_TO, serde_json::json!({ "asset_id": id, "longest_mm": mm }));
    };
    let flatten = move || {
        let (Some(id), Some(mm)) = (selected_id(), parse_positive(&cut_mm.get_untracked())) else {
            return;
        };
        cut_preview.set(false);
        run_op(cmd::MESH_FLATTEN, serde_json::json!({ "asset_id": id, "height_mm": mm }));
    };
    let mirror = move || {
        if let Some(id) = selected_id() {
            run_op(cmd::MESH_MIRROR, serde_json::json!({ "asset_id": id }));
        }
    };
    let export = move |format: &'static str| {
        let Some(id) = selected_id() else { return };
        spawn_local(async move {
            let Some(dest) = ipc::pick_save_path("Export", &format!("model.{format}"), format).await else {
                return;
            };
            let args = serde_json::json!({ "asset_id": id, "format": format, "dest_path": dest });
            match ipc::call_unit(cmd::EXPORT_MESH, &args).await {
                Ok(()) => state.notify_info(td_string!(current_locale(), lab.exported)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let open_external = move || {
        let Some(id) = selected_id() else { return };
        spawn_local(async move {
            if let Err(e) = ipc::call_unit(cmd::OPEN_ASSET_EXTERNAL, &serde_json::json!({ "id": id })).await {
                state.notify_error(e);
            }
        });
    };
    let remove = move |id: String| {
        spawn_local(async move {
            match ipc::call_unit(cmd::DELETE_ASSET, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    assets.update(|list| list.retain(|a| a.id != id));
                    if selected.with_untracked(|s| s.as_ref().is_some_and(|a| a.id == id)) {
                        selected.set(assets.with_untracked(|l| l.first().cloned()));
                    }
                }
                Err(e) => state.notify_error(e),
            }
        });
    };

    // ---- 3D 视图 ----
    // 只挂载一次(Effect 在视图进入 DOM 之后才跑,此时容器已存在)
    Effect::new(move |prev: Option<()>| {
        if prev.is_some() {
            return;
        }
        let dark = state.theme.get_untracked() == Theme::Dark;
        spawn_local(async move {
            match viewer3d::mount(VIEWER_ID, dark).await {
                Ok(()) => viewer_ready.set(true),
                Err(e) => {
                    crate::main_log("lab", &format!("viewer mount failed: {e}"));
                    viewer_error.set(Some(e));
                }
            }
        });
    });
    on_cleanup(|| viewer3d::dispose(VIEWER_ID));

    // 选中的模型变了 → 让视图加载它
    Effect::new(move |_| {
        if !viewer_ready.get() {
            return;
        }
        let Some((id, ext)) = selected.with(|a| a.as_ref().map(|a| (a.id.clone(), a.ext.clone()))) else {
            return;
        };
        spawn_local(async move {
            match viewer3d::load_model(VIEWER_ID, &ipc::asset_url(&id), &ext, false).await {
                Ok(s) => stats.set(s),
                Err(e) => {
                    crate::main_log("lab", &format!("load {id}.{ext} failed: {e}"));
                    state.notify_error(format!("{}: {e}", td_string!(current_locale(), lab.viewer_failed)));
                }
            }
        });
    });
    Effect::new(move |_| {
        if viewer_ready.get() {
            viewer3d::set_display(VIEWER_ID, wireframe.get(), show_bed.get());
        }
    });
    Effect::new(move |_| {
        if viewer_ready.get() {
            let h = parse_positive(&cut_mm.get()).unwrap_or(0.0);
            // selected 也要跟踪:换了模型,切面要按新模型的包围盒重新摆
            let _ = selected.with(|a| a.as_ref().map(|a| a.id.clone()));
            viewer3d::set_cut_plane(VIEWER_ID, cut_preview.get(), h);
        }
    });
    Effect::new(move |_| {
        if viewer_ready.get() {
            viewer3d::set_dark(VIEWER_ID, state.theme.get() == Theme::Dark);
        }
    });
    // 每秒取一次帧率
    if let Ok(handle) = set_interval_with_handle(
        move || {
            if viewer_ready.get_untracked() {
                let fresh = viewer3d::stats(VIEWER_ID);
                stats.update(|s| s.fps = fresh.fps);
            }
        },
        std::time::Duration::from_secs(1),
    ) {
        on_cleanup(move || handle.clear());
    }

    let label = move |f: fn(crate::i18n::Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    let no_selection = Signal::derive(move || busy.get() || selected.with(Option::is_none));
    // 带泛型尖括号的表达式不能直接写在 view! 的属性里(会被当成标签解析),先在外面算好
    let detail_options: Vec<(u32, Signal<String>)> = (2..=7u32).map(|d| (d, Signal::stored(d.to_string()))).collect();

    view! {
        <div class="h-full flex flex-col">
            <div class="flex-1 min-h-0 flex">
                // ---- 左:模型列表 ----
                <aside class="w-64 shrink-0 h-full flex flex-col border-r border-gray-200 dark:border-gray-700">
                    <div class="p-3 space-y-3 border-b border-gray-200 dark:border-gray-700">
                        <div class="flex items-center justify-between gap-2">
                            <Segmented
                                value=Signal::derive(move || shape.get())
                                options=vec![
                                    (Shape::Sphere, label(|l| leptos_i18n::td_string!(l, lab.shape_sphere))),
                                    (Shape::Torus, label(|l| leptos_i18n::td_string!(l, lab.shape_torus))),
                                    (Shape::Cuboid, label(|l| leptos_i18n::td_string!(l, lab.shape_box))),
                                ]
                                on_change=move |s: Shape| shape.set(s)
                            />
                        </div>
                        <div class="flex items-center justify-between gap-2 text-xs text-gray-500 dark:text-gray-400">
                            <span>{move || t_string!(i18n, lab.detail)}</span>
                            <Segmented
                                value=Signal::derive(move || detail.get())
                                options=detail_options
                                on_change=move |d: u32| detail.set(d)
                            />
                        </div>
                        <div class="flex gap-2">
                            <Button small=true icon=IconKind::Sparkles disabled=busy on_click=generate>
                                {move || t_string!(i18n, lab.generate)}
                            </Button>
                            <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Upload disabled=busy on_click=import>
                                {move || t_string!(i18n, lab.import)}
                            </Button>
                        </div>
                    </div>
                    <div class="flex-1 min-h-0 overflow-y-auto p-2 space-y-1">
                        <Show when=move || assets.with(Vec::is_empty)>
                            <p class="p-3 text-xs leading-relaxed text-gray-400">{move || t_string!(i18n, lab.no_models)}</p>
                        </Show>
                        <For each=move || assets.get() key=|a| a.id.clone() let:asset>
                            <ModelRow asset=asset selected=selected on_remove=remove/>
                        </For>
                    </div>
                </aside>

                // ---- 中:3D 视图 ----
                <section class="flex-1 min-w-0 h-full flex flex-col">
                    <div class="shrink-0 h-10 px-3 flex items-center gap-4 border-b border-gray-200 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400">
                        <label class="flex items-center gap-1.5">
                            <Toggle checked=wireframe on_change=move |v: bool| wireframe.set(v)/>
                            {move || t_string!(i18n, lab.wireframe)}
                        </label>
                        <label class="flex items-center gap-1.5">
                            <Toggle checked=show_bed on_change=move |v: bool| show_bed.set(v)/>
                            {move || t_string!(i18n, lab.show_bed)}
                        </label>
                        <label class="flex items-center gap-1.5">
                            <Toggle checked=cut_preview on_change=move |v: bool| cut_preview.set(v)/>
                            {move || t_string!(i18n, lab.cut_preview)}
                        </label>
                        <div class="flex-1"></div>
                        <Show when=move || busy.get()>
                            <Badge tone=Tone::Brand>{move || t_string!(i18n, common.working)}</Badge>
                        </Show>
                        <span class="tabular-nums">
                            {move || {
                                let s = stats.get();
                                (s.load_ms > 0 || s.fps > 0)
                                    .then(|| t_string!(i18n, lab.viewer_stats, ms = s.load_ms, fps = s.fps).to_string())
                            }}
                        </span>
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
                                <EmptyState icon=IconKind::Box title=move || t_string!(i18n, lab.pick_model)/>
                            </div>
                        </Show>
                    </div>
                </section>

                // ---- 右:分析与编辑 ----
                <aside class="w-72 shrink-0 h-full overflow-y-auto border-l border-gray-200 dark:border-gray-700 p-3 space-y-3">
                    <Card class="p-4 space-y-3">
                        <SectionTitle title=move || t_string!(i18n, lab.report)/>
                        {move || report.get().map(|r| view! { <ReportTable report=r/> })}
                    </Card>

                    <Card class="p-4 space-y-3">
                        <SectionTitle title=move || t_string!(i18n, lab.tools)/>
                        <Field label=move || t_string!(i18n, lab.scale_label)>
                            <div class="flex gap-2">
                                <TextInput value=scale_mm numeric=true on_enter=scale/>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Ruler disabled=no_selection on_click=scale>
                                    {move || t_string!(i18n, common.apply)}
                                </Button>
                            </div>
                        </Field>
                        <Field label=move || t_string!(i18n, lab.flatten_label)>
                            <div class="flex gap-2">
                                <TextInput value=cut_mm numeric=true on_enter=flatten/>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Scissors disabled=no_selection on_click=flatten>
                                    {move || t_string!(i18n, common.apply)}
                                </Button>
                            </div>
                        </Field>
                        <div class="flex flex-wrap gap-2 pt-1">
                            <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Flip disabled=no_selection on_click=mirror>
                                {move || t_string!(i18n, lab.mirror)}
                            </Button>
                        </div>
                    </Card>

                    <Card class="p-4 space-y-2">
                        <div class="flex flex-wrap gap-2">
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
        </div>
    }
}

#[component]
fn ModelRow(asset: Asset, selected: RwSignal<Option<Asset>>, #[prop(into)] on_remove: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(asset.id.clone());
    let stored = StoredValue::new(asset.clone());
    let active = move || selected.with(|s| s.as_ref().is_some_and(|a| id.with_value(|id| &a.id == id)));
    let report = report_of(&asset);
    let tris = report.as_ref().map(|r| fmt_int(r.triangles)).unwrap_or_default();
    let closed = report.as_ref().map(|r| r.manifold());
    view! {
        <div
            class="group flex items-center gap-2 px-2.5 py-2 rounded-lg cursor-pointer transition-colors"
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| selected.set(Some(stored.get_value()))
        >
            <Icon kind=IconKind::Box class="w-4 h-4 shrink-0 text-gray-400"/>
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-1.5">
                    <span class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{asset.role.clone()}</span>
                    <span class="text-[10px] uppercase text-gray-400">{asset.ext.clone()}</span>
                </div>
                <div class="flex items-center gap-1.5 text-[11px] text-gray-400 tabular-nums">
                    <span>{tris}</span>
                    {closed.map(|ok| if ok {
                        view! { <Badge tone=Tone::Green>{move || t_string!(i18n, lab.manifold)}</Badge> }.into_any()
                    } else {
                        view! { <Badge tone=Tone::Red>"!"</Badge> }.into_any()
                    })}
                </div>
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

#[component]
fn ReportTable(report: MeshReport) -> impl IntoView {
    let i18n = use_i18n();
    let size = report.size();
    let estimate = report.estimate(PrintEstimateParams::default());
    let fits = report.fits(BUILD_VOLUME);
    let yes_no = move |ok: bool| {
        if ok {
            view! { <Badge tone=Tone::Green>{move || t_string!(i18n, common.yes)}</Badge> }
        } else {
            view! { <Badge tone=Tone::Red>{move || t_string!(i18n, common.no)}</Badge> }
        }
    };
    let row = move |label: Signal<String>, value: AnyView| {
        view! {
            <div class="flex items-center justify-between gap-3 py-1 text-xs">
                <span class="text-gray-500 dark:text-gray-400">{move || label.get()}</span>
                <span class="tabular-nums text-gray-900 dark:text-gray-100 selectable">{value}</span>
            </div>
        }
    };
    let label = move |f: fn(crate::i18n::Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    let count = |n: u32| {
        if n == 0 {
            "0".into_any()
        } else {
            view! { <span class="text-red-600 dark:text-red-400 font-medium">{fmt_int(n)}</span> }.into_any()
        }
    };

    view! {
        <div>
            {row(
                label(|l| leptos_i18n::td_string!(l, lab.size)),
                format!("{} × {} × {} mm", fmt_mm(size[0]), fmt_mm(size[1]), fmt_mm(size[2])).into_any(),
            )}
            {row(label(|l| leptos_i18n::td_string!(l, lab.volume)), format!("{:.1} cm³", report.volume_cm3()).into_any())}
            {row(label(|l| leptos_i18n::td_string!(l, lab.area)), format!("{:.1} cm²", report.area_mm2 / 100.0).into_any())}
            {row(label(|l| leptos_i18n::td_string!(l, lab.triangles)), fmt_int(report.triangles).into_any())}
            {row(label(|l| leptos_i18n::td_string!(l, lab.components)), report.components.to_string().into_any())}
            {row(label(|l| leptos_i18n::td_string!(l, lab.watertight)), yes_no(report.watertight()).into_any())}
            {row(label(|l| leptos_i18n::td_string!(l, lab.manifold)), yes_no(report.manifold()).into_any())}
            {row(label(|l| leptos_i18n::td_string!(l, lab.boundary_edges)), count(report.boundary_edges))}
            {row(label(|l| leptos_i18n::td_string!(l, lab.non_manifold_edges)), count(report.non_manifold_edges))}
            {row(label(|l| leptos_i18n::td_string!(l, lab.flipped_edges)), count(report.flipped_edges))}
            {row(label(|l| leptos_i18n::td_string!(l, lab.fits_bed)), yes_no(fits).into_any())}
        </div>
        <div class="pt-2 border-t border-gray-100 dark:border-gray-700 space-y-1">
            <div class="text-xs font-medium text-gray-700 dark:text-gray-200">{move || t_string!(i18n, lab.estimate)}</div>
            <div class="text-sm tabular-nums text-gray-900 dark:text-gray-100">
                {move || t_string!(
                    i18n,
                    lab.estimate_value,
                    grams = format!("{:.0}", estimate.grams),
                    hours = format!("{:.1}", estimate.hours),
                ).to_string()}
            </div>
            <p class="text-[11px] leading-relaxed text-gray-400">{move || t_string!(i18n, lab.estimate_hint)}</p>
        </div>
    }
}
