//! 网格命令:导入、度量、缩放、切平底、镜像、导出。
//!
//! **预览与落盘分离**(docs/03-architecture.md §8.1):前端的 three.js 只做视觉预览,
//! 用户点「应用」才调这里;每次操作产出一个**新资产**(父子血缘),原资产不动。

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use pp_common::mesh::{MeshOpResult, MeshReport};
use pp_common::{errcode, AssetKind};
use pp_db::NewAsset;
use pp_geometry::io::{read_mesh, write_3mf, write_stl, IoError};
use pp_geometry::{analyze, cut_below, shapes, CutError, Mesh};
use tauri::State;

use super::join_err;
use crate::AppCtx;

fn io_err(e: IoError) -> String {
    errcode::err(errcode::MESH_UNREADABLE, e)
}

fn cut_err(e: CutError) -> String {
    match e {
        CutError::NothingAbove => errcode::err(errcode::MESH_EMPTY_RESULT, e),
        CutError::OpenContour { .. } | CutError::Triangulation(_) => errcode::err(errcode::MESH_NOT_CLOSED, e),
    }
}

fn fs_err(e: std::io::Error) -> String {
    errcode::err(errcode::IO_FAILED, e)
}

/// 资产在资料库里的子目录:属于项目的放 `assets/<项目编号>/`,其余放 `assets/lab/`。
fn folder_for(ctx: &AppCtx, project_id: Option<&str>) -> Result<String, String> {
    match project_id {
        Some(id) => Ok(ctx.db.get_project(id).map_err(|e| e.to_wire())?.code),
        None => Ok("lab".to_string()),
    }
}

fn load_mesh(ctx: &AppCtx, asset_id: &str) -> Result<(pp_common::Asset, Mesh), String> {
    let asset = ctx.db.get_asset(asset_id).map_err(|e| e.to_wire())?;
    let bytes = std::fs::read(ctx.asset_path(&asset.rel_path)).map_err(fs_err)?;
    let mesh = read_mesh(&bytes, &asset.ext).map_err(io_err)?;
    Ok((asset, mesh))
}

/// 把一张网格存成新的 STL 资产,并把分析报告写进 `meta_json`。
fn save_mesh(
    ctx: &AppCtx,
    mesh: &Mesh,
    role: &str,
    project_id: Option<String>,
    parent: Option<String>,
    started: Instant,
) -> Result<MeshOpResult, String> {
    let report = analyze(mesh);
    let bytes = write_stl(mesh);
    let mut new = NewAsset::new(AssetKind::Model3d, role, "", "stl", bytes.len() as i64);
    new.rel_path = format!("assets/{}/{}.stl", folder_for(ctx, project_id.as_deref())?, new.id);
    new.project_id = project_id;
    new.parent_asset_id = parent;
    new.meta_json = serde_json::to_string(&report).ok();

    let path = ctx.asset_path(&new.rel_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fs_err)?;
    }
    std::fs::write(&path, bytes).map_err(fs_err)?;
    let asset = ctx.db.insert_asset(&new).map_err(|e| e.to_wire())?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    log::info!(
        "[mesh] {role} → {} · {} 面 · 水密 {} · {elapsed_ms}ms",
        asset.rel_path,
        report.triangles,
        report.watertight()
    );
    Ok(MeshOpResult {
        asset,
        report,
        elapsed_ms,
    })
}

/// 生成一个样例模型(演示模式 / 预研用):`sphere` | `torus` | `box`。`detail` 越大面数越多。
#[tauri::command(rename_all = "snake_case")]
pub async fn lab_generate_sample(
    ctx: State<'_, Arc<AppCtx>>,
    shape: String,
    detail: u32,
) -> Result<MeshOpResult, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let mut mesh = match shape.as_str() {
            "sphere" => shapes::icosphere(30.0, detail.min(8)),
            "torus" => {
                let seg = (24 * 2u32.pow(detail.min(5))).max(24);
                shapes::torus(30.0, 10.0, seg, seg / 2)
            }
            "box" => shapes::cuboid([40.0, 30.0, 20.0]),
            other => return Err(errcode::err(errcode::INVALID_INPUT, format!("unknown shape {other}"))),
        };
        mesh.place_on_bed();
        save_mesh(&ctx, &mesh, "lab_sample", None, None, started)
    })
    .await
    .map_err(join_err)?
}

/// 导入用户自己的模型文件(STL / GLB)。原文件原样拷进资料库——GLB 的贴图留着,预览更好看。
#[tauri::command(rename_all = "snake_case")]
pub async fn import_model(
    ctx: State<'_, Arc<AppCtx>>,
    path: String,
    project_id: Option<String>,
) -> Result<MeshOpResult, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let src = Path::new(&path);
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let bytes = std::fs::read(src).map_err(fs_err)?;
        // 先确认读得懂,再入库:不让一个打不开的文件变成资产
        let mesh = read_mesh(&bytes, &ext).map_err(io_err)?;
        let report = analyze(&mesh);

        let mut new = NewAsset::new(AssetKind::Model3d, "model_raw", "", &ext, bytes.len() as i64);
        new.rel_path = format!("assets/{}/{}.{ext}", folder_for(&ctx, project_id.as_deref())?, new.id);
        new.project_id = project_id;
        new.meta_json = serde_json::to_string(&report).ok();
        let dest = ctx.asset_path(&new.rel_path);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(fs_err)?;
        }
        std::fs::write(&dest, &bytes).map_err(fs_err)?;
        let asset = ctx.db.insert_asset(&new).map_err(|e| e.to_wire())?;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        log::info!(
            "[mesh] 导入 {} ({:.1} MB) · {} 面 · 水密 {} · {elapsed_ms}ms",
            src.display(),
            bytes.len() as f64 / 1_048_576.0,
            report.triangles,
            report.watertight()
        );
        Ok(MeshOpResult {
            asset,
            report,
            elapsed_ms,
        })
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mesh_report(ctx: State<'_, Arc<AppCtx>>, asset_id: String) -> Result<MeshReport, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (_, mesh) = load_mesh(&ctx, &asset_id)?;
        Ok(analyze(&mesh))
    })
    .await
    .map_err(join_err)?
}

/// 等比缩放到「最长边 = longest_mm」并贴床。AI 生成的模型没有单位,这一步给它定尺寸。
#[tauri::command(rename_all = "snake_case")]
pub async fn mesh_scale_to(
    ctx: State<'_, Arc<AppCtx>>,
    asset_id: String,
    longest_mm: f64,
) -> Result<MeshOpResult, String> {
    if !(longest_mm.is_finite() && longest_mm > 0.0 && longest_mm <= 2000.0) {
        return Err(errcode::err(errcode::INVALID_INPUT, "longest_mm out of range"));
    }
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let (asset, mut mesh) = load_mesh(&ctx, &asset_id)?;
        mesh.scale_to_longest(longest_mm);
        mesh.place_on_bed();
        save_mesh(&ctx, &mesh, "model_edited", asset.project_id, Some(asset.id), started)
    })
    .await
    .map_err(join_err)?
}

/// 切平底:从最低点往上 `height_mm` 处水平切一刀,封口,再贴床。
#[tauri::command(rename_all = "snake_case")]
pub async fn mesh_flatten(
    ctx: State<'_, Arc<AppCtx>>,
    asset_id: String,
    height_mm: f64,
) -> Result<MeshOpResult, String> {
    if !(height_mm.is_finite() && height_mm > 0.0) {
        return Err(errcode::err(errcode::INVALID_INPUT, "height_mm must be positive"));
    }
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let (asset, mut mesh) = load_mesh(&ctx, &asset_id)?;
        mesh.place_on_bed();
        let mut cut = cut_below(&mesh, height_mm).map_err(cut_err)?;
        cut.place_on_bed();
        save_mesh(&ctx, &cut, "model_edited", asset.project_id, Some(asset.id), started)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mesh_mirror(ctx: State<'_, Arc<AppCtx>>, asset_id: String) -> Result<MeshOpResult, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let (asset, mut mesh) = load_mesh(&ctx, &asset_id)?;
        mesh.mirror_x();
        mesh.place_on_bed();
        save_mesh(&ctx, &mesh, "model_edited", asset.project_id, Some(asset.id), started)
    })
    .await
    .map_err(join_err)?
}

/// 导出到用户选的位置。`format`: `stl` | `3mf`。
#[tauri::command(rename_all = "snake_case")]
pub async fn export_mesh(
    ctx: State<'_, Arc<AppCtx>>,
    asset_id: String,
    format: String,
    dest_path: String,
) -> Result<(), String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (asset, mesh) = load_mesh(&ctx, &asset_id)?;
        let bytes = match format.as_str() {
            "stl" => write_stl(&mesh),
            "3mf" => write_3mf(&mesh, &asset.role).map_err(|e| errcode::err(errcode::IO_FAILED, e))?,
            other => return Err(errcode::err(errcode::INVALID_INPUT, format!("unknown format {other}"))),
        };
        std::fs::write(&dest_path, bytes).map_err(fs_err)?;
        log::info!("[mesh] 导出 {} → {dest_path}", asset.rel_path);
        Ok(())
    })
    .await
    .map_err(join_err)?
}
