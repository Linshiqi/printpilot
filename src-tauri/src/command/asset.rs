use std::sync::Arc;

use pp_common::{errcode, Asset, AssetKind};
use tauri::http::{header, Response};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::AppCtx;

fn mime_of(ext: &str) -> &'static str {
    match ext {
        "glb" => "model/gltf-binary",
        "stl" => "model/stl",
        "3mf" => "model/3mf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

fn status(code: u16) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Vec::new())
        .expect("static response")
}

/// 资产 ID 是 UUID:只含十六进制与连字符。其余一律拒绝,连查库都不去查。
fn is_asset_id(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// `pp-asset://<asset_id>` 的处理函数(在独立线程里跑,见 lib.rs)。
pub fn serve(app: &AppHandle, id: &str) -> Response<Vec<u8>> {
    if !is_asset_id(id) {
        return status(400);
    }
    let Some(ctx) = app.try_state::<Arc<AppCtx>>() else {
        return status(503); // setup 还没跑完
    };
    let Ok(asset) = ctx.db.get_asset(id) else {
        return status(404);
    };
    match std::fs::read(ctx.asset_path(&asset.rel_path)) {
        Ok(data) => Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, mime_of(&asset.ext))
            // three.js 的加载器用 fetch 取模型,页面源是 tauri.localhost,需要放行跨源
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            // 资产不可变(编辑 = 生成新资产),可以放心缓存
            .header(header::CACHE_CONTROL, "private, max-age=31536000, immutable")
            .body(data)
            .expect("asset response"),
        Err(e) => {
            log::warn!("[asset] {} 读不到: {e}", asset.rel_path);
            status(404)
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_assets(
    ctx: State<'_, Arc<AppCtx>>,
    project_id: Option<String>,
    kind: Option<AssetKind>,
) -> Result<Vec<Asset>, String> {
    ctx.db
        .list_assets(project_id.as_deref(), kind)
        .map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_asset(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    // 只做软删除,文件留在资料库里(将来的「回收站 / 清理」统一处理)
    ctx.db.delete_asset(&id).map_err(|e| e.to_wire())
}

/// 用系统关联的程序打开资产文件——模型文件就是交给切片软件(docs/04-integrations.md §6.1)。
#[tauri::command(rename_all = "snake_case")]
pub async fn open_asset_external(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    let asset = ctx.db.get_asset(&id).map_err(|e| e.to_wire())?;
    let path = ctx.asset_path(&asset.rel_path);
    app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| errcode::err(errcode::IO_FAILED, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_uuid_shaped_ids_are_accepted() {
        assert!(is_asset_id("0192f3a4-7b1c-7def-8a90-1234567890ab"));
        for bad in ["", "short", "../../etc/passwd", "0192f3a4%2F..", "0192f3a4-7b1c-7def-8a90-1234567890ab/x", "C:\\x"] {
            assert!(!is_asset_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn models_get_model_mime_types() {
        assert_eq!(mime_of("glb"), "model/gltf-binary");
        assert_eq!(mime_of("stl"), "model/stl");
        assert_eq!(mime_of("exe"), "application/octet-stream");
    }
}
