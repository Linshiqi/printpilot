use std::sync::Arc;

use pp_common::{errcode, AppInfo};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::AppCtx;

fn build_info(app: &AppHandle, ctx: &AppCtx) -> Result<AppInfo, String> {
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        library_dir: ctx.library_dir.display().to_string(),
        db_path: ctx.db.path().display().to_string(),
        schema_version: ctx.db.schema_version().map_err(|e| e.to_wire())?,
        project_count: ctx.db.project_count().map_err(|e| e.to_wire())?,
        demo_mode: ctx.config().demo_mode,
        config_writable: ctx.config_writable(),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn app_info(app: AppHandle, ctx: State<'_, Arc<AppCtx>>) -> Result<AppInfo, String> {
    build_info(&app, &ctx)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_demo_mode(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, enabled: bool) -> Result<AppInfo, String> {
    ctx.update_config(|c| c.demo_mode = enabled)
        .map_err(|e| errcode::err(errcode::IO_FAILED, e))?;
    log::info!("[settings] 演示模式 → {enabled}");
    build_info(&app, &ctx)
}

/// 前端启动耗时埋点(velo 做法):wasm 起来用了多久、首屏挂载用了多久。
#[tauri::command(rename_all = "snake_case")]
pub fn log_boot(label: String, wasm_ms: f64, mount_ms: f64, bundle: String) {
    log::info!("[boot] 窗口 {label}: wasm 就绪 {wasm_ms:.0}ms,挂载 {mount_ms:.0}ms,bundle {bundle}");
}

/// 前端未捕获异常 / panic 上报。前端侧已有配额,这里只管落日志。
#[tauri::command(rename_all = "snake_case")]
pub fn log_client_error(label: String, message: String) {
    log::warn!("[fe-error] [{label}] {message}");
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reveal_library(app: AppHandle, ctx: State<'_, Arc<AppCtx>>) -> Result<(), String> {
    app.opener()
        .open_path(ctx.library_dir.display().to_string(), None::<&str>)
        .map_err(|e| errcode::err(errcode::IO_FAILED, e))
}
