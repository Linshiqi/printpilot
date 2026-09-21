//! 代码式 CAD 命令(docs/adr/0003-code-cad-build123d.md)。
//!
//! 这里是「引擎这一侧」:引擎状态与安装、真执行器(常驻进程)、执行产物入库、参考图处理、演示脚本、导出。
//! 面向用户的建模命令(设计、对话、时间线)在 `command/design.rs`,它复用这里的内部函数。
//!
//! 每次成功都存成一个新版本(`cad_versions`,父子关系 = 版本树),STL / STEP 作为资产入库。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use pp_agent::{AgentError, CadConfig, CadExecutor, CadProgress};
use pp_cad::{engine_info, parse_params, CadError, Engine, RunOptions, RunOutput, Worker};
use pp_common::cad::{CadBuildReport, CadEngineInfo, CadMetrics, CadVersion, DesignSpec};
use pp_common::{errcode, Asset, AssetKind};
use pp_db::{NewAsset, NewCadVersion};
use pp_providers::llm::{image_data_url, LlmPricing, LlmProvider, MockLlm};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use super::join_err;
use super::provider::llm_for;
use crate::engine_pack;
use crate::AppCtx;

/// 进度事件名(前端 listen 这个)。载荷:`{ step, attempt?, problems? }`
const PROGRESS_EVENT: &str = "cad-progress";
/// 引擎包解包进度。载荷:`{ phase: verifying|extracting|done, done, total }`
const ENGINE_PROGRESS_EVENT: &str = "cad-engine-progress";
/// 一次看图最多带几张参考图 / 渲染图
pub(crate) const MAX_REFERENCE_IMAGES: usize = 4;
const MAX_RENDER_IMAGES: usize = 6;
/// 参考图入库时缩到的最长边(像素)。视觉模型每张图的 token 有上限,再大也是白传
const REFERENCE_MAX_SIDE: u32 = 1536;
const MAX_CODE_BYTES: usize = 200_000;
/// 一个常驻引擎进程最多跑多少个任务就换新的(OpenCascade 长跑会涨内存)
const WORKER_MAX_JOBS: u32 = 40;
/// 等常驻进程就绪的上限:冷盘上 import build123d 可能要十几秒
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(90);

pub(crate) fn cad_err(e: CadError) -> String {
    errcode::err(e.code(), e)
}

pub(crate) fn agent_err(e: AgentError) -> String {
    errcode::err(e.code(), e)
}

pub(crate) fn fs_err(e: std::io::Error) -> String {
    errcode::err(errcode::IO_FAILED, e)
}

pub(crate) fn invalid(detail: impl std::fmt::Display) -> String {
    errcode::err(errcode::INVALID_INPUT, detail)
}

fn progress_payload(p: &CadProgress) -> serde_json::Value {
    match p {
        CadProgress::ReadingImage => json!({ "step": "reading_image" }),
        CadProgress::WritingCode { attempt } => json!({ "step": "writing_code", "attempt": attempt }),
        CadProgress::Running { attempt } => json!({ "step": "running", "attempt": attempt }),
        CadProgress::Repairing { attempt, problems } => json!({ "step": "repairing", "attempt": attempt, "problems": problems }),
        CadProgress::Reviewing => json!({ "step": "reviewing" }),
        CadProgress::Done => json!({ "step": "done" }),
    }
}

pub(crate) fn emitter(app: &AppHandle) -> impl Fn(CadProgress) + Send + Sync {
    let app = app.clone();
    move |p: CadProgress| {
        let _ = app.emit(PROGRESS_EVENT, progress_payload(&p));
    }
}

// ---------------------------------------------------------------- 引擎执行器

/// 建模用的临时目录都在这下面(每次命令一个子目录、常驻进程一个沙箱)。
fn scratch_root() -> PathBuf {
    std::env::temp_dir().join("printpilot-cad")
}

/// 启动时清掉上次留下的临时目录。正常情况下它们随 Drop 删除,但应用退出时析构函数不会跑
///(常驻进程的沙箱就是这样留下来的)。单实例保证了这时没有别的进程在用它们。
pub fn clean_scratch() {
    let root = scratch_root();
    if root.exists() {
        match std::fs::remove_dir_all(&root) {
            Ok(()) => log::info!("[cad] 清理了上次留下的临时目录"),
            Err(e) => log::warn!("[cad] 临时目录 {} 没清干净:{e}", root.display()),
        }
    }
}

/// 一次命令用的临时目录:每次执行在里面开一个子目录,命令结束(Drop)整个删掉。
struct Session {
    dir: PathBuf,
}

impl Session {
    fn new() -> Result<Self, String> {
        let dir = scratch_root().join(pp_db::new_id());
        std::fs::create_dir_all(&dir).map_err(fs_err)?;
        Ok(Self { dir })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn worker_sandbox() -> PathBuf {
    scratch_root().join(format!("worker-{}", pp_db::new_id()))
}

/// 确保有一个可用的常驻进程(没有就起一个)。起不来返回 `None`,调用方退回冷启动。
fn ensure_worker(slot: &mut Option<Worker>, engine: &Engine) -> bool {
    if slot.as_ref().is_some_and(|w| w.served() >= WORKER_MAX_JOBS) {
        *slot = None;
    }
    if slot.is_none() {
        let started = Instant::now();
        match Worker::spawn(engine, &worker_sandbox(), WORKER_READY_TIMEOUT) {
            Ok(w) => {
                log::info!("[cad] 常驻引擎就绪 · build123d {} · {}ms", w.build123d_version, started.elapsed().as_millis());
                *slot = Some(w);
            }
            Err(e) => log::warn!("[cad] 常驻引擎起不来,退回单次执行:{e}"),
        }
    }
    slot.is_some()
}

/// 优先在常驻进程里执行(几十毫秒);常驻进程起不来就退回冷启动(约 4 秒)。
fn run_code(ctx: &AppCtx, engine: &Engine, code: &str, dir: &Path, opts: &RunOptions) -> Result<RunOutput, CadError> {
    let mut slot = ctx.cad_worker.lock().unwrap_or_else(|e| e.into_inner());
    if !ensure_worker(&mut slot, engine) {
        drop(slot);
        return pp_cad::run(engine, code, dir, opts);
    }
    let mut worker = slot.take().expect("ensure_worker said there is one");
    let result = worker.run(code, dir, opts);
    // 脚本自己的错误不影响进程;超时 / 协议错误之后进程已经被杀,丢掉,下次再起
    if matches!(result, Ok(_) | Err(CadError::Script(_))) {
        *slot = Some(worker);
    }
    result
}

/// 真引擎执行器。流水线交付的不一定是最后一次执行的那版(它挑问题最少的),所以每次成功的产物都按代码留着。
pub(crate) struct EngineExecutor {
    ctx: Arc<AppCtx>,
    engine: Engine,
    session: Session,
    opts: RunOptions,
    counter: AtomicU32,
    outputs: Mutex<HashMap<String, RunOutput>>,
}

impl EngineExecutor {
    pub(crate) fn new(ctx: &Arc<AppCtx>) -> Result<Self, String> {
        let engine = Engine::locate(&ctx.data_root).ok_or_else(|| cad_err(CadError::EngineMissing))?;
        Ok(Self {
            ctx: ctx.clone(),
            engine,
            session: Session::new()?,
            opts: RunOptions::default(),
            counter: AtomicU32::new(0),
            outputs: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn take_output(&self, code: &str) -> Option<RunOutput> {
        self.outputs.lock().unwrap_or_else(|e| e.into_inner()).remove(code)
    }
}

#[async_trait]
impl CadExecutor for EngineExecutor {
    async fn execute(&self, code: &str) -> Result<CadMetrics, CadError> {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let dir = self.session.dir.join(format!("run-{n}"));
        let (ctx, engine, opts, owned) = (self.ctx.clone(), self.engine.clone(), self.opts.clone(), code.to_string());
        let output = tauri::async_runtime::spawn_blocking(move || run_code(&ctx, &engine, &owned, &dir, &opts))
            .await
            .map_err(|e| CadError::Protocol(format!("engine task failed: {e}")))??;
        let metrics = output.metrics.clone();
        self.outputs.lock().unwrap_or_else(|e| e.into_inner()).insert(code.to_string(), output);
        Ok(metrics)
    }
}

// ---------------------------------------------------------------- 入库

pub(crate) fn folder_for(ctx: &AppCtx, project_id: Option<&str>) -> Result<String, String> {
    match project_id {
        Some(id) => Ok(ctx.db.get_project(id).map_err(|e| e.to_wire())?.code),
        None => Ok("lab".to_string()),
    }
}

fn store_file(
    ctx: &AppCtx,
    src: &Path,
    kind: AssetKind,
    role: &str,
    ext: &str,
    project_id: Option<&str>,
    parent: Option<&str>,
    ai_generated: bool,
    meta_json: Option<String>,
) -> Result<Asset, String> {
    let bytes = std::fs::metadata(src).map_err(fs_err)?.len() as i64;
    let mut new = NewAsset::new(kind, role, "", ext, bytes);
    new.rel_path = format!("assets/{}/{}.{ext}", folder_for(ctx, project_id)?, new.id);
    new.project_id = project_id.map(str::to_string);
    new.parent_asset_id = parent.map(str::to_string);
    new.ai_generated = ai_generated;
    new.meta_json = meta_json;
    let dest = ctx.asset_path(&new.rel_path);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(fs_err)?;
    }
    std::fs::copy(src, &dest).map_err(fs_err)?;
    ctx.db.insert_asset(&new).map_err(|e| e.to_wire())
}

/// 这一版「是怎么来的」。
pub(crate) struct Lineage {
    pub design_id: Option<String>,
    pub project_id: Option<String>,
    pub parent: Option<CadVersion>,
    pub source: &'static str,
    pub note: String,
    pub spec: Option<DesignSpec>,
    pub ref_asset_ids: Vec<String>,
    pub report: Option<CadBuildReport>,
}

/// 执行产物 → 资产 + 版本。STL 顺带做一次网格分析写进 `meta_json`——它是一个普通的模型资产,「模型」页的工具都能接着用。
pub(crate) fn persist(ctx: &AppCtx, code: &str, output: &RunOutput, lineage: Lineage) -> Result<CadVersion, String> {
    let stl_path = output
        .files
        .get("stl")
        .ok_or_else(|| cad_err(CadError::Protocol("engine produced no STL".into())))?;
    let project_id = lineage.project_id.as_deref();
    let parent_stl = lineage.parent.as_ref().map(|p| p.stl_asset_id.clone());
    // 模型写的代码 = AI 生成;手写代码与纯改参数沿用父版本的属性
    let ai = matches!(lineage.source, "generate" | "edit" | "repair");

    let mesh_meta = std::fs::read(stl_path)
        .ok()
        .and_then(|bytes| pp_geometry::io::read_mesh(&bytes, "stl").ok())
        .and_then(|mesh| serde_json::to_string(&pp_geometry::analyze(&mesh)).ok());
    let stl = store_file(ctx, stl_path, AssetKind::Model3d, "cad_mesh", "stl", project_id, parent_stl.as_deref(), ai, mesh_meta)?;
    let step = match output.files.get("step") {
        Some(path) => Some(store_file(ctx, path, AssetKind::Model3d, "cad_step", "step", project_id, Some(&stl.id), ai, None)?),
        None => None,
    };

    let version = ctx
        .db
        .insert_cad_version(&NewCadVersion {
            design_id: lineage.design_id.clone(),
            project_id: lineage.project_id.clone(),
            parent_id: lineage.parent.as_ref().map(|p| p.id.clone()),
            source: lineage.source.to_string(),
            note: lineage.note,
            code: code.to_string(),
            spec: lineage.spec,
            ref_asset_ids: lineage.ref_asset_ids,
            report: lineage.report,
            params: parse_params(code),
            metrics: output.metrics.clone(),
            stl_asset_id: stl.id,
            step_asset_id: step.map(|s| s.id),
            elapsed_ms: output.elapsed_ms,
        })
        .map_err(|e| e.to_wire())?;
    log::info!(
        "[cad] {} → 版本 {} · {:.1} × {:.1} × {:.1} mm · {} 个面 · {}ms",
        version.source,
        version.id,
        version.metrics.size[0],
        version.metrics.size[1],
        version.metrics.size[2],
        version.metrics.faces,
        version.elapsed_ms
    );
    Ok(version)
}

pub(crate) fn check_code(code: &str) -> Result<(), String> {
    if code.trim().is_empty() {
        return Err(invalid("code is empty"));
    }
    if code.len() > MAX_CODE_BYTES {
        return Err(invalid("code is too long"));
    }
    Ok(())
}

// ---------------------------------------------------------------- 参考图

/// 参考图入库前的处理:按 EXIF 摆正 → 缩到最长边 1536 → 透明底铺白 → 重新编码成 JPEG。
/// 重新编码同时去掉了 EXIF(手机照片里有 GPS 和机型)——这张图之后要发给第三方的视觉模型。
pub(crate) fn prepare_reference(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    use image::{DynamicImage, ImageDecoder, ImageReader};
    let unreadable = |e: image::ImageError| errcode::err(errcode::IMAGE_UNREADABLE, e);

    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    let orientation = decoder.orientation().map_err(unreadable)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > REFERENCE_MAX_SIDE {
        img = img.resize(REFERENCE_MAX_SIDE, REFERENCE_MAX_SIDE, image::imageops::FilterType::Triangle);
    }

    // JPEG 没有透明通道:直接丢掉 alpha 会让透明底变成一片黑,模型会把它当成物体的一部分
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(w, h);
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let a = src[3] as u32;
        for c in 0..3 {
            dst[c] = ((src[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode_image(&rgb)
        .map_err(unreadable)?;
    Ok((out, w, h))
}

/// 参考图资产 → data URL。只认图片资产。
pub(crate) fn reference_data_urls(ctx: &AppCtx, ids: &[String]) -> Result<Vec<String>, String> {
    if ids.len() > MAX_REFERENCE_IMAGES {
        return Err(invalid(format!("at most {MAX_REFERENCE_IMAGES} reference images")));
    }
    ids.iter()
        .map(|id| {
            let asset = ctx.db.get_asset(id).map_err(|e| e.to_wire())?;
            if !matches!(asset.kind, AssetKind::Image | AssetKind::Photo) {
                return Err(invalid(format!("asset {id} is not an image")));
            }
            let bytes = std::fs::read(ctx.asset_path(&asset.rel_path)).map_err(fs_err)?;
            Ok(image_data_url(&bytes, &asset.ext))
        })
        .collect()
}

/// 前端截的渲染图:只收 `data:image/…;base64,`,并限制张数与大小。
pub(crate) fn check_renders(renders: &[String]) -> Result<(), String> {
    if renders.is_empty() || renders.len() > MAX_RENDER_IMAGES {
        return Err(invalid(format!("expected 1 to {MAX_RENDER_IMAGES} render images")));
    }
    for r in renders {
        let ok = (r.starts_with("data:image/png;base64,") || r.starts_with("data:image/jpeg;base64,")) && r.len() < 6_000_000;
        if !ok {
            return Err(invalid("render images must be PNG / JPEG data URLs under 4 MB"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 演示模式

/// 演示用的设计规格与脚本。脚本是真的 build123d 代码,在真引擎上跑——演示模式只是不联网、不花钱。
pub(crate) const DEMO_SPEC: &str = r#"{
  "name": "三槽线缆夹(演示)",
  "summary": "演示数据:放在桌沿的线缆夹,三道线槽,底部留双面胶位。",
  "suitable": true,
  "overall_mm": [60, 24, 16],
  "features": [
    {"name": "body", "description": "圆角矩形主体,底面贴床", "dimensions": {"length": 60, "width": 24, "height": 16, "corner_radius": 4}},
    {"name": "cable_slots", "description": "顶面 3 道沿 Y 方向贯通的线槽,沿 X 等距", "dimensions": {"count": 3, "slot_width": 6, "slot_depth": 9}},
    {"name": "slot_entry", "description": "线槽开口处的倒角,方便把线压进去", "dimensions": {"chamfer": 1}}
  ],
  "assumptions": ["演示数据:尺寸为常见桌面线缆夹的估计值"],
  "print_notes": "线槽开口朝上,无需支撑"
}"#;

pub(crate) const DEMO_CODE: &str = r#"from build123d import *

# ---- PARAMS ----
length = 60.0        # mm | 总长 | [30, 200]
width = 24.0         # mm | 总宽 | [12, 80]
height = 16.0        # mm | 总高 | [8, 60]
corner_radius = 4.0  # mm | 四角圆角 | [0.5, 10]
slot_count = 3       # 个 | 线槽数量 | [1, 8]
slot_width = 6.0     # mm | 线槽宽度 | [2, 15]
slot_depth = 9.0     # mm | 线槽深度 | [2, 40]
entry_chamfer = 1.0  # mm | 槽口倒角 | [0.2, 2]

# ---- FEATURE: body ----
body = extrude(RectangleRounded(length, width, radius=corner_radius), amount=height)

# ---- FEATURE: cable_slots ----
pitch = length / (slot_count + 1)
slot_xs = [-length / 2 + pitch * (i + 1) for i in range(slot_count)]
slot_tool = Box(slot_width, width + 2, slot_depth + 1, align=(Align.CENTER, Align.CENTER, Align.MIN))
body = body - [Pos(x, 0, height - slot_depth) * slot_tool for x in slot_xs]

# ---- FEATURE: slot_entry ----
top_edges = body.edges().group_by(Axis.Z)[-1].filter_by(Axis.Y)
body = chamfer(top_edges, length=entry_chamfer)

# ---- RESULT ----
result = body
"#;

/// 演示的第一版故意带一个真实会发生的错误(圆角比壁厚还大),让「报错 → 喂回去 → 修好」这条路在界面上看得见。
pub(crate) fn demo_broken_code() -> String {
    DEMO_CODE.replace("length=entry_chamfer)", "length=entry_chamfer * 40)")
}

/// 演示的「指令修补」:不管指令是什么,都只在末尾加一段「底边倒角」(FDM 上用来抵消第一层的「象脚」外扩)。
pub(crate) fn demo_edit(code: &str) -> String {
    format!(
        "{}\n\n# ---- FEATURE: demo_bottom_chamfer ----\n# 演示模式:不理解指令内容,固定演示「只新增一个特征段、其余逐字不动」\nresult = chamfer(result.edges().group_by(Axis.Z)[0], length=0.6)\n",
        code.trim_end()
    )
}

pub(crate) fn fenced(code: &str) -> String {
    format!("```python\n{code}\n```")
}

fn free(cfg: &mut CadConfig) {
    let zero = LlmPricing {
        input_cache_hit: 0.0,
        input_cache_miss: 0.0,
        output: 0.0,
    };
    cfg.vision_pricing = zero;
    cfg.code_pricing = zero;
}

pub(crate) fn cad_config(demo: bool) -> CadConfig {
    let mut cfg = CadConfig::default();
    if demo {
        free(&mut cfg);
    }
    cfg
}

pub(crate) fn llm_or_demo(ctx: &AppCtx, demo_answers: Vec<String>) -> Result<Box<dyn LlmProvider>, String> {
    if ctx.config().demo_mode {
        Ok(Box::new(MockLlm::new(demo_answers)))
    } else {
        Ok(Box::new(llm_for(ctx)?))
    }
}

// ---------------------------------------------------------------- 命令

/// 安装包里带的引擎包(正式版都带;开发构建只有一个占位文件)。
fn bundled_pack(app: &AppHandle) -> Option<(PathBuf, engine_pack::PackManifest)> {
    engine_pack::bundled(&app.path().resource_dir().ok()?)
}

/// 带着引擎包、而数据目录里解开的不是这个版本 → 需要(重新)安装。
/// 开发者用环境变量显式指定了解释器时不插手。
fn pack_to_install(app: &AppHandle, ctx: &AppCtx) -> Option<(PathBuf, engine_pack::PackManifest)> {
    if std::env::var_os(pp_cad::engine::ENV_OVERRIDE).is_some_and(|v| !v.is_empty()) {
        return None;
    }
    let (archive, manifest) = bundled_pack(app)?;
    let installed = engine_pack::installed_version(&ctx.data_root.join("cad-engine"));
    (installed.as_deref() != Some(manifest.engine_version.as_str())).then_some((archive, manifest))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cad_engine_info(app: AppHandle, ctx: State<'_, Arc<AppCtx>>) -> Result<CadEngineInfo, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bundled = bundled_pack(&app).is_some();
        if let Some((_, manifest)) = pack_to_install(&app, &ctx) {
            // 界面看到 needs_install 会直接调 cad_engine_install,不用用户点
            return CadEngineInfo {
                bundled: true,
                needs_install: true,
                unpacked_bytes: manifest.unpacked_bytes,
                problem: "engine pack is not unpacked yet".into(),
                ..Default::default()
            };
        }
        let Some(engine) = Engine::locate(&ctx.data_root) else {
            return CadEngineInfo {
                bundled,
                ..engine_info(&ctx.data_root)
            };
        };
        // 探测引擎本来就要起一次解释器、import 一次 build123d(几秒):干脆起成常驻进程,
        // 版本号从它的就绪行里拿——面板打开之后,第一次建模就是热的。
        let mut slot = ctx.cad_worker.lock().unwrap_or_else(|e| e.into_inner());
        if !ensure_worker(&mut slot, &engine) {
            drop(slot);
            // 冷探测一次,把起不来的真实原因带回去
            return CadEngineInfo {
                bundled,
                ..engine_info(&ctx.data_root)
            };
        }
        let worker = slot.as_ref().expect("ensure_worker said there is one");
        CadEngineInfo {
            available: true,
            python: engine.python.display().to_string(),
            python_version: worker.python_version.clone(),
            build123d_version: worker.build123d_version.clone(),
            bundled,
            ..Default::default()
        }
    })
    .await
    .map_err(join_err)
}

/// 把安装包里带的引擎包解到应用数据目录(首次使用、或应用升级后引擎版本变了)。约一分钟,进度走事件。
#[tauri::command(rename_all = "snake_case")]
pub async fn cad_engine_install(app: AppHandle, ctx: State<'_, Arc<AppCtx>>) -> Result<(), String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 排队:后到的请求等前一个装完,再看一眼——这时已经是装好的了,直接返回
        let _one_at_a_time = ctx.engine_install.lock().unwrap_or_else(|e| e.into_inner());
        let Some((archive, manifest)) = pack_to_install(&app, &ctx) else {
            return Ok(()); // 已经是这个版本了(或者这个构建根本不带引擎包)
        };
        // 正在运行的解释器在 Windows 上删不掉:先让常驻进程退出
        *ctx.cad_worker.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let started = Instant::now();
        let root = ctx.data_root.join("cad-engine");
        let emit = {
            let app = app.clone();
            move |p: engine_pack::Progress| {
                let _ = app.emit(ENGINE_PROGRESS_EVENT, p);
            }
        };
        let files = engine_pack::install(&archive, &manifest, &root, &pp_db::new_id(), &emit)
            .map_err(|e| errcode::err(errcode::CAD_ENGINE_FAILED, e))?;
        log::info!(
            "[cad] 引擎包 {} 已解开:{files} 个文件 · {:.0} MB · {:.1}s",
            manifest.engine_version,
            manifest.unpacked_bytes as f64 / 1e6,
            started.elapsed().as_secs_f64()
        );
        Ok(())
    })
    .await
    .map_err(join_err)?
}

/// 导出到用户选的位置。`format`:`step`(源头真值,能进任何 CAD)| `stl` | `3mf`。
#[tauri::command(rename_all = "snake_case")]
pub async fn cad_export(ctx: State<'_, Arc<AppCtx>>, version_id: String, format: String, dest_path: String) -> Result<(), String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let version = ctx.db.get_cad_version(&version_id).map_err(|e| e.to_wire())?;
        let path_of = |asset_id: &str| -> Result<PathBuf, String> {
            let asset = ctx.db.get_asset(asset_id).map_err(|e| e.to_wire())?;
            Ok(ctx.asset_path(&asset.rel_path))
        };
        match format.as_str() {
            "stl" => {
                std::fs::copy(path_of(&version.stl_asset_id)?, &dest_path).map_err(fs_err)?;
            }
            "step" => {
                let id = version
                    .step_asset_id
                    .as_deref()
                    .ok_or_else(|| errcode::err(errcode::NOT_FOUND, "this version has no STEP file"))?;
                std::fs::copy(path_of(id)?, &dest_path).map_err(fs_err)?;
            }
            "3mf" => {
                let bytes = std::fs::read(path_of(&version.stl_asset_id)?).map_err(fs_err)?;
                let mesh = pp_geometry::io::read_mesh(&bytes, "stl").map_err(|e| errcode::err(errcode::MESH_UNREADABLE, e))?;
                let name = version.spec.as_ref().map(|s| s.name.as_str()).unwrap_or("model");
                let out = pp_geometry::io::write_3mf(&mesh, name).map_err(|e| errcode::err(errcode::IO_FAILED, e))?;
                std::fs::write(&dest_path, out).map_err(fs_err)?;
            }
            other => return Err(invalid(format!("unknown format {other}"))),
        }
        log::info!("[cad] 导出 {} ({format}) → {dest_path} · {}ms", version.id, started.elapsed().as_millis());
        Ok(())
    })
    .await
    .map_err(join_err)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_agent::{edit_model, generate_model};
    use pp_cad::contract::changed_sections;

    #[test]
    fn progress_payloads_carry_a_step_name() {
        assert_eq!(progress_payload(&CadProgress::ReadingImage)["step"], "reading_image");
        let p = progress_payload(&CadProgress::Repairing { attempt: 2, problems: 3 });
        assert_eq!((p["step"].as_str(), p["attempt"].as_u64(), p["problems"].as_u64()), (Some("repairing"), Some(2), Some(3)));
    }

    #[test]
    fn renders_must_be_small_image_data_urls() {
        let ok = "data:image/png;base64,AAAA".to_string();
        assert!(check_renders(&[ok.clone()]).is_ok());
        assert!(check_renders(&[]).is_err());
        assert!(check_renders(&vec![ok.clone(); MAX_RENDER_IMAGES + 1]).is_err());
        assert!(check_renders(&["https://example.com/a.png".to_string()]).is_err(), "不替前端去抓任意网址");
        assert!(check_renders(&["data:text/html;base64,AAAA".to_string()]).is_err());
    }

    #[test]
    fn reference_images_are_shrunk_flattened_onto_white_and_re_encoded() {
        // 一张 3000 × 1500、左半透明的 PNG
        let mut src = image::RgbaImage::new(3000, 1500);
        for (x, _, px) in src.enumerate_pixels_mut() {
            *px = if x < 1500 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([200, 30, 30, 255]) };
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(src)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let (jpeg, w, h) = prepare_reference(&png).unwrap();
        assert_eq!((w, h), (1536, 768), "最长边缩到 1536,比例不变");
        assert_eq!(&jpeg[..3], [0xFF, 0xD8, 0xFF], "输出是 JPEG");
        let back = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let left = back.get_pixel(100, 400);
        assert!(left[0] > 240 && left[1] > 240 && left[2] > 240, "透明底要铺成白色,不能变黑:{left:?}");
        let right = back.get_pixel(1400, 400);
        assert!(right[0] > 150 && right[1] < 90, "{right:?}");

        assert!(prepare_reference(b"not an image").unwrap_err().starts_with("#image_unreadable#"));
    }

    #[test]
    fn the_demo_spec_parses_and_the_demo_edit_touches_exactly_one_new_section() {
        let spec: DesignSpec = serde_json::from_str(DEMO_SPEC).expect("演示规格可解析");
        assert!(spec.suitable && spec.features.len() == 3);
        assert!(spec.summary.contains("演示数据"), "演示数据必须自己说明自己是演示数据");
        assert_eq!(parse_params(DEMO_CODE).len(), 8);
        assert!(pp_cad::contract::contract_issues(DEMO_CODE).is_empty());
        assert_ne!(demo_broken_code(), DEMO_CODE, "「故意写坏」的替换要真的生效");
        assert_eq!(changed_sections(DEMO_CODE, &demo_edit(DEMO_CODE)), ["demo_bottom_chamfer"]);
    }

    // ---- 以下需要真引擎(scripts/setup-cad-engine.ps1);没装就跳过 ----

    struct RealExec {
        engine: Engine,
        session: Session,
        counter: AtomicU32,
    }

    #[async_trait]
    impl CadExecutor for RealExec {
        async fn execute(&self, code: &str) -> Result<CadMetrics, CadError> {
            let dir = self.session.dir.join(format!("run-{}", self.counter.fetch_add(1, Ordering::Relaxed)));
            pp_cad::run(&self.engine, code, &dir, &RunOptions::default()).map(|o| o.metrics)
        }
    }

    fn real_exec() -> Option<RealExec> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        let engine = Engine::locate(&Path::new(&local).join(crate::APP_IDENTIFIER));
        if engine.is_none() {
            eprintln!("(skipped: no CAD engine installed — run scripts/setup-cad-engine.ps1)");
        }
        Some(RealExec {
            engine: engine?,
            session: Session::new().unwrap(),
            counter: AtomicU32::new(0),
        })
    }

    #[tokio::test]
    async fn the_demo_script_goes_through_the_real_pipeline_on_the_real_engine() {
        let Some(exec) = real_exec() else { return };
        let spec: DesignSpec = serde_json::from_str(DEMO_SPEC).unwrap();
        let llm = MockLlm::new([fenced(&demo_broken_code()), fenced(DEMO_CODE)]);
        let build = generate_model(&llm, &exec, &spec, &cad_config(true), &|_| {}).await.unwrap();

        let metrics = build.metrics.expect("演示脚本必须能在真引擎上建成");
        assert_eq!(build.report.rounds.len(), 1, "第一版是故意写坏的:{:?}", build.report.rounds);
        assert!(
            matches!(build.report.rounds[0].as_slice(), [pp_common::cad::CadProblem::Script { error }] if error.stage == "exec" && error.line.is_some()),
            "真引擎的报错要带阶段和行号:{:?}",
            build.report.rounds[0]
        );
        assert!(build.report.warnings.is_empty(), "修好之后应当完全合格:{:?}", build.report.warnings);
        assert_eq!(metrics.solids, 1);
        assert!(metrics.is_valid);
        for (got, want) in metrics.size.iter().zip([60.0, 24.0, 16.0]) {
            assert!((got - want).abs() < 0.01, "{:?}", metrics.size);
        }
        assert_eq!(build.report.cost_fen, 0.0, "演示模式不记费用");
    }

    #[tokio::test]
    async fn the_demo_edit_builds_on_the_real_engine_and_only_adds_its_own_section() {
        let Some(exec) = real_exec() else { return };
        let llm = MockLlm::new([fenced(&demo_edit(DEMO_CODE))]);
        let build = edit_model(&llm, &exec, DEMO_CODE, "底边加个小倒角", None, &cad_config(true), &|_| {})
            .await
            .unwrap();
        assert!(build.metrics.is_some(), "{:?}", build.report.rounds);
        assert_eq!(build.report.changed_sections, ["demo_bottom_chamfer"]);
        assert!(build.report.warnings.is_empty(), "{:?}", build.report.warnings);
    }
}
