//! 在线升级(docs/adr/0010-online-update.md)。整体照搬 velo 的做法:`tauri-plugin-updater` + 自己的两条命令。
//!
//! 升级下载的是**精简包**(不带 224 MB 的建模引擎,十几 MB):引擎解在应用数据目录里、有自己的版本号,
//! 应用升级不动它;哪天应用要求新的引擎版本、而安装目录里又没有带包,才按需单独下载(`command::cad`)。
//!
//! 更新清单的地址、验签用的公钥都在 `tauri.conf.json` 的 `plugins.updater` 里,不写死在代码里。
//! 下载到的包由插件按公钥验签;验不过不会安装。

use std::time::Duration;

use pp_common::errcode;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// 检查更新只是读一个几百字节的 JSON:等久了没有意义
const CHECK_TIMEOUT: Duration = Duration::from_secs(12);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);
/// 下载进度事件。载荷:`{ downloaded, total }`(总数未知时是 0)
const PROGRESS_EVENT: &str = "update-progress";

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    pub notes: String,
}

fn updater(app: &AppHandle, timeout: Duration) -> Result<tauri_plugin_updater::Updater, String> {
    app.updater_builder().timeout(timeout).build().map_err(|e| errcode::err(errcode::UPDATE_CHECK_FAILED, e))
}

/// 有没有新版本。`None` = 已经是最新。查不到(断网、地址还没配好)是一个普通的错误,界面上轻轻说一声就行。
#[tauri::command(rename_all = "snake_case")]
pub async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    match updater(&app, CHECK_TIMEOUT)?.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            version: update.version.clone(),
            current: update.current_version.clone(),
            notes: update.body.clone().unwrap_or_default(),
        })),
        Ok(None) => Ok(None),
        Err(e) => {
            log::warn!("[update] 检查更新失败:{e}");
            Err(errcode::err(errcode::UPDATE_CHECK_FAILED, e))
        }
    }
}

/// 下载并安装。Windows 上安装程序接手之后应用会自己退出;macOS 上装完重启应用。
#[tauri::command(rename_all = "snake_case")]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let update = updater(&app, INSTALL_TIMEOUT)?
        .check()
        .await
        .map_err(|e| errcode::err(errcode::UPDATE_CHECK_FAILED, e))?
        .ok_or_else(|| errcode::err(errcode::UPDATE_CHECK_FAILED, "already up to date"))?;
    log::info!("[update] {} → {}:开始下载", update.current_version, update.version);

    let progress_app = app.clone();
    let downloaded = std::sync::atomic::AtomicU64::new(0);
    update
        .download_and_install(
            move |chunk, total| {
                let done = downloaded.fetch_add(chunk as u64, std::sync::atomic::Ordering::Relaxed) + chunk as u64;
                let _ = progress_app.emit(PROGRESS_EVENT, serde_json::json!({ "downloaded": done, "total": total.unwrap_or(0) }));
            },
            || log::info!("[update] 下载完成,开始安装"),
        )
        .await
        .map_err(|e| {
            log::warn!("[update] 安装失败:{e}");
            errcode::err(errcode::UPDATE_INSTALL_FAILED, e)
        })?;

    #[cfg(not(target_os = "windows"))]
    app.restart();
    #[cfg(target_os = "windows")]
    Ok(())
}
