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
    pub const CANCEL_TURN: &str = "cancel_turn";

    pub const LIST_PROJECTS: &str = "list_projects";
    pub const CREATE_PROJECT: &str = "create_project";
    #[allow(dead_code)]
    pub const GET_PROJECT: &str = "get_project";
    pub const UPDATE_PROJECT: &str = "update_project";
    pub const MOVE_PROJECT_STAGE: &str = "move_project_stage";
    pub const SET_PROJECT_STATUS: &str = "set_project_status";
    pub const DELETE_PROJECT: &str = "delete_project";
    pub const LIST_STAGE_EVENTS: &str = "list_stage_events";
    pub const PROJECT_FACTS_ALL: &str = "project_facts_all";
    pub const PROJECT_OVERVIEW: &str = "project_overview";

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
    pub const CAD_EXPORT: &str = "cad_export";

    pub const DESIGN_LIST: &str = "design_list";
    pub const DESIGN_CREATE: &str = "design_create";
    pub const DESIGN_GET: &str = "design_get";
    pub const DESIGN_RENAME: &str = "design_rename";
    pub const DESIGN_DELETE: &str = "design_delete";
    pub const DESIGN_LINK_PROJECT: &str = "design_link_project";
    pub const DESIGN_SET_THUMB: &str = "design_set_thumb";
    pub const DESIGN_SELECT_VERSION: &str = "design_select_version";
    pub const DESIGN_ADD_IMAGE: &str = "design_add_image";
    pub const DESIGN_REMOVE_REFERENCE: &str = "design_remove_reference";
    pub const DESIGN_SEND: &str = "design_send";
    pub const DESIGN_GENERATE: &str = "design_generate";
    pub const DESIGN_SET_PARAM: &str = "design_set_param";
    pub const DESIGN_RUN_CODE: &str = "design_run_code";
    pub const DESIGN_REVIEW: &str = "design_review";

    pub const IMAGE_PROVIDER_INFO: &str = "image_provider_info";
    pub const IMAGE_SETTINGS_GET: &str = "image_settings_get";
    pub const IMAGE_SETTINGS_SET: &str = "image_settings_set";
    pub const BOARD_LIST: &str = "board_list";
    pub const BOARD_CREATE: &str = "board_create";
    pub const BOARD_GET: &str = "board_get";
    pub const BOARD_RENAME: &str = "board_rename";
    pub const BOARD_DELETE: &str = "board_delete";
    pub const BOARD_SET_OPTIONS: &str = "board_set_options";
    pub const BOARD_LINK_PROJECT: &str = "board_link_project";
    pub const BOARD_SELECT: &str = "board_select";
    pub const BOARD_ADOPT: &str = "board_adopt";
    pub const BOARD_REMOVE_IMAGE: &str = "board_remove_image";
    pub const BOARD_ADD_IMAGE: &str = "board_add_image";
    pub const BOARD_SEND: &str = "board_send";
    pub const BOARD_TO_DESIGN: &str = "board_to_design";
    pub const BOARD_EXPORT: &str = "board_export";

    pub const PRINTER_LIST: &str = "printer_list";
    pub const PRINTER_SAVE: &str = "printer_save";
    pub const PRINTER_DELETE: &str = "printer_delete";
    pub const MATERIAL_LIST: &str = "material_list";
    pub const MATERIAL_SAVE: &str = "material_save";
    pub const MATERIAL_DELETE: &str = "material_delete";
    pub const COST_DEFAULTS_GET: &str = "cost_defaults_get";
    pub const COST_DEFAULTS_SET: &str = "cost_defaults_set";
    pub const PRICING_GET: &str = "pricing_get";
    pub const PRICING_SAVE: &str = "pricing_save";
    pub const PRICING_SUMMARY: &str = "pricing_summary";
    pub const PRINT_RUN_ADD: &str = "print_run_add";
    pub const PRINT_RUN_DELETE: &str = "print_run_delete";

    pub const OPEN_URL: &str = "plugin:opener|open_url";
}

/// 后端推给前端的事件名(kebab-case,与后端 `app.emit` 的名字一一对应)。
pub mod event {
    pub const RESEARCH_PROGRESS: &str = "research-progress";
    pub const CAD_PROGRESS: &str = "cad-progress";
    pub const CAD_STREAM: &str = "cad-stream";
    pub const CAD_ENGINE_PROGRESS: &str = "cad-engine-progress";
    pub const IMAGE_PROGRESS: &str = "image-progress";
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
