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

/// 读系统剪贴板里的文字——右键菜单的「粘贴」。WebView 自己读剪贴板(`navigator.clipboard.readText`)
/// 在 Windows 上会弹权限框,所以走后端(velo 的做法)。剪贴板里不是文字(图片、文件)时返回空串,不算错。
#[tauri::command(rename_all = "snake_case")]
pub async fn read_clipboard_text() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| errcode::err(errcode::IO_FAILED, e))?;
        Ok(clipboard.get_text().unwrap_or_default())
    })
    .await
    .map_err(|e| errcode::err(errcode::IO_FAILED, e))?
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

/// 取消正在跑的一轮(建模对话 / 出图 / 调研)。`scope`:`design:<id>` / `board:<id>` / `research`。
/// 返回有没有取消到东西——那一轮可能刚好已经结束了,这不算错。
#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_turn(ctx: State<'_, Arc<AppCtx>>, scope: String) -> Result<bool, String> {
    let hit = ctx.turns.cancel(&scope);
    log::info!("[turn] 取消 {scope}:{}", if hit { "已通知" } else { "没有在跑的" });
    Ok(hit)
}
