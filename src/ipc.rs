//! 前端调后端的唯一通道(约定照搬 velo):
//! - 命令名集中在 `cmd` 常量表,调用处只写 `cmd::XXX`,与后端 `#[tauri::command]` 的函数名一一对应;
//! - 参数用 `serde_json::json!({...})`,键名 snake_case;
//! - 返回 `Result<T, String>`,错误串已经按当前语言本地化过,可以直接弹 toast。

use serde::{de::DeserializeOwned, Serialize};
use wasm_bindgen::prelude::*;

pub mod cmd {
    pub const APP_INFO: &str = "app_info";
    pub const LOG_BOOT: &str = "log_boot";
    pub const LOG_CLIENT_ERROR: &str = "log_client_error";
    pub const REVEAL_LIBRARY: &str = "reveal_library";
    pub const SET_DEMO_MODE: &str = "set_demo_mode";

    pub const LIST_PROJECTS: &str = "list_projects";
    pub const CREATE_PROJECT: &str = "create_project";
    #[allow(dead_code)]
    pub const GET_PROJECT: &str = "get_project";
    pub const UPDATE_PROJECT: &str = "update_project";
    pub const MOVE_PROJECT_STAGE: &str = "move_project_stage";
    pub const SET_PROJECT_STATUS: &str = "set_project_status";
    pub const DELETE_PROJECT: &str = "delete_project";
    pub const LIST_STAGE_EVENTS: &str = "list_stage_events";

    pub const LIST_ASSETS: &str = "list_assets";
    pub const DELETE_ASSET: &str = "delete_asset";
    pub const OPEN_ASSET_EXTERNAL: &str = "open_asset_external";

    pub const LAB_GENERATE_SAMPLE: &str = "lab_generate_sample";
    pub const IMPORT_MODEL: &str = "import_model";
    #[allow(dead_code)]
    pub const MESH_REPORT: &str = "mesh_report";
    pub const MESH_SCALE_TO: &str = "mesh_scale_to";
    pub const MESH_FLATTEN: &str = "mesh_flatten";
    pub const MESH_MIRROR: &str = "mesh_mirror";
    pub const EXPORT_MESH: &str = "export_mesh";

    pub const PROVIDER_STATUS: &str = "provider_status";
    pub const SAVE_PROVIDER_KEY: &str = "save_provider_key";
    pub const DELETE_PROVIDER_KEY: &str = "delete_provider_key";
    pub const TEST_PROVIDER: &str = "test_provider";

    pub const RESEARCH_RUN: &str = "research_run";
    #[allow(dead_code)]
    pub const LIST_RESEARCH_RUNS: &str = "list_research_runs";
    #[allow(dead_code)]
    pub const GET_RESEARCH: &str = "get_research";
    pub const ADOPT_OPPORTUNITY: &str = "adopt_opportunity";

    pub const CAD_ENGINE_INFO: &str = "cad_engine_info";
    pub const CAD_ENGINE_INSTALL: &str = "cad_engine_install";
    pub const CAD_IMPORT_REFERENCE: &str = "cad_import_reference";
    pub const CAD_DESIGN_SPEC: &str = "cad_design_spec";
    pub const CAD_GENERATE: &str = "cad_generate";
    pub const CAD_EDIT: &str = "cad_edit";
    pub const CAD_REVIEW: &str = "cad_review";
    pub const CAD_RUN: &str = "cad_run";
    pub const CAD_SET_PARAM: &str = "cad_set_param";
    pub const CAD_LIST_VERSIONS: &str = "cad_list_versions";
    #[allow(dead_code)]
    pub const CAD_GET_VERSION: &str = "cad_get_version";
    pub const CAD_DELETE_VERSION: &str = "cad_delete_version";
    pub const CAD_EXPORT: &str = "cad_export";

    pub const OPEN_URL: &str = "plugin:opener|open_url";
}

/// 后端推给前端的事件名(kebab-case,与后端 `app.emit` 的名字一一对应)。
pub mod event {
    pub const RESEARCH_PROGRESS: &str = "research-progress";
    pub const CAD_PROGRESS: &str = "cad-progress";
    pub const CAD_ENGINE_PROGRESS: &str = "cad-engine-progress";
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    fn invoke_now(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = convertFileSrc)]
    fn convert_file_src(path: &str, protocol: &str) -> String;

    #[allow(dead_code)]
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], catch)]
    pub async fn listen(event: &str, handler: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "dialog"], js_name = open, catch)]
    async fn dialog_open(options: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "dialog"], js_name = save, catch)]
    async fn dialog_save(options: JsValue) -> Result<JsValue, JsValue>;
}

/// 用「JSON 兼容」的序列化器:map → 普通对象、None → null。
/// 默认序列化器会把 `serde_json::json!` 的对象变成 ES `Map`;Tauri 的 IPC 恰好会把 Map 转回对象,
/// 所以默认的也能用——但我们不依赖这个隐式行为。
fn to_js<A: Serialize + ?Sized>(args: &A) -> Result<JsValue, String> {
    args.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| format!("failed to serialize arguments: {e}"))
}

fn from_js<R: DeserializeOwned>(v: JsValue) -> Result<R, String> {
    serde_wasm_bindgen::from_value(v).map_err(|e| format!("failed to deserialize response: {e}"))
}

pub fn fmt_invoke_err(e: &JsValue) -> String {
    let raw = e.as_string().unwrap_or_else(|| format!("{e:?}"));
    crate::i18n_util::localize_backend_err(&raw)
}

/// 发出去就不管了(日志上报用):不等结果,失败也不报。
pub fn fire<A: Serialize + ?Sized>(command: &str, args: &A) {
    if let Ok(js) = to_js(args) {
        let _ = invoke_now(command, js);
    }
}

pub async fn call<A, R>(command: &str, args: &A) -> Result<R, String>
where
    A: Serialize + ?Sized,
    R: DeserializeOwned,
{
    let res = invoke(command, to_js(args)?).await.map_err(|e| fmt_invoke_err(&e))?;
    from_js(res)
}

pub async fn call_unit<A: Serialize + ?Sized>(command: &str, args: &A) -> Result<(), String> {
    invoke(command, to_js(args)?).await.map_err(|e| fmt_invoke_err(&e))?;
    Ok(())
}

pub async fn call_no_args<R: DeserializeOwned>(command: &str) -> Result<R, String> {
    let res = invoke(command, JsValue::NULL).await.map_err(|e| fmt_invoke_err(&e))?;
    from_js(res)
}

pub async fn call_unit_no_args(command: &str) -> Result<(), String> {
    invoke(command, JsValue::NULL).await.map_err(|e| fmt_invoke_err(&e))?;
    Ok(())
}

/// 资产 ID → WebView 可以直接加载的 URL(Windows 上是 `http://pp-asset.localhost/<id>`)。
pub fn asset_url(asset_id: &str) -> String {
    convert_file_src(asset_id, "pp-asset")
}

/// 原生「打开文件」对话框。用户取消返回 `None`。
pub async fn pick_file(title: &str, extensions: &[&str]) -> Option<String> {
    let opts = serde_json::json!({
        "title": title,
        "multiple": false,
        "directory": false,
        "filters": [{ "name": title, "extensions": extensions }],
    });
    dialog_open(to_js(&opts).ok()?).await.ok()?.as_string()
}

/// 原生「另存为」对话框。用户取消返回 `None`。
pub async fn pick_save_path(title: &str, default_name: &str, extension: &str) -> Option<String> {
    let opts = serde_json::json!({
        "title": title,
        "defaultPath": default_name,
        "filters": [{ "name": extension.to_uppercase(), "extensions": [extension] }],
    });
    dialog_save(to_js(&opts).ok()?).await.ok()?.as_string()
}
