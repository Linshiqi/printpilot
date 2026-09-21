//! three.js 视图桥的 Rust 绑定(模式照搬 velo 的 `pdf_view.rs` ↔ `public/pdfjs/bridge.mjs`)。
//!
//! JS 侧只渲染:Leptos 出一个空容器 `<div id=…>`,把 id 传过去,JS 往里挂 canvas。
//! 所有函数按容器 id 寻址,所以同一个页面里可以有多个视图。

use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/public/viewer3d/bridge.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = "mount", catch)]
    async fn js_mount(container_id: &str, opts_json: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "loadModel", catch)]
    async fn js_load_model(container_id: &str, url: &str, format: &str, keep_view: bool) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "setDisplay")]
    pub fn set_display(container_id: &str, wireframe: bool, show_bed: bool);

    #[wasm_bindgen(js_name = "setCutPlane")]
    pub fn set_cut_plane(container_id: &str, enabled: bool, height_mm: f64);

    #[wasm_bindgen(js_name = "setDark")]
    pub fn set_dark(container_id: &str, dark: bool);

    #[wasm_bindgen(js_name = "stats")]
    fn js_stats(container_id: &str) -> String;

    #[allow(dead_code)]
    #[wasm_bindgen(js_name = "snapshot")]
    pub fn snapshot(container_id: &str) -> String;

    /// 镜头回到默认视角并把模型框满
    #[wasm_bindgen(js_name = "resetView")]
    pub fn reset_view(container_id: &str);

    /// 棱线叠加(CAD 零件的孔、槽、倒角靠它才看得清)
    #[wasm_bindgen(js_name = "setEdges")]
    pub fn set_edges(container_id: &str, enabled: bool);

    #[wasm_bindgen(js_name = "setPickMode")]
    fn js_set_pick_mode(container_id: &str, enabled: bool, handler: &JsValue);

    #[wasm_bindgen(js_name = "clearPick")]
    pub fn clear_pick(container_id: &str);

    #[wasm_bindgen(js_name = "snapshotViews")]
    fn js_snapshot_views(container_id: &str, size: u32) -> String;

    #[wasm_bindgen(js_name = "dispose")]
    pub fn dispose(container_id: &str);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerStats {
    #[serde(default)]
    pub fps: u32,
    #[serde(default)]
    pub load_ms: u32,
    #[serde(default)]
    pub triangles: u32,
}

fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| format!("{e:?}"))
}

/// 第一次调用时才会去取 three.js(约 600 KB);之后的调用直接用缓存的模块。
pub async fn mount(container_id: &str, dark: bool) -> Result<(), String> {
    let opts = format!(r#"{{"dark":{dark}}}"#);
    js_mount(container_id, &opts).await.map(|_| ()).map_err(js_err)
}

/// `keep_view`:保持当前视角(改参数后重新加载同一个零件时用)。
pub async fn load_model(container_id: &str, url: &str, format: &str, keep_view: bool) -> Result<ViewerStats, String> {
    let v = js_load_model(container_id, url, format, keep_view).await.map_err(js_err)?;
    Ok(parse_stats(&v.as_string().unwrap_or_default()))
}

/// 在模型上点中的位置(模型坐标系,毫米,Z 朝上)。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Pick {
    pub point: [f64; 3],
    pub normal: [f64; 3],
}

/// 点选回调的持有者:它活着,回调就有效;丢掉它(组件卸载)回调随之失效。
pub struct PickHandler(#[allow(dead_code)] Closure<dyn Fn(String)>);

/// 打开点选。返回的 `PickHandler` 要由调用方存着(`StoredValue::new_local`)。
pub fn enable_pick(container_id: &str, on_pick: impl Fn(Pick) + 'static) -> PickHandler {
    let closure = Closure::<dyn Fn(String)>::new(move |json: String| {
        if let Ok(p) = serde_json::from_str::<Pick>(&json) {
            on_pick(p);
        }
    });
    js_set_pick_mode(container_id, true, closure.as_ref());
    PickHandler(closure)
}

pub fn disable_pick(container_id: &str) {
    js_set_pick_mode(container_id, false, &JsValue::NULL);
}

/// 看图复核用的多视角截图(JPEG data URL):等轴测、正面、顶面、右侧。
pub fn snapshot_views(container_id: &str, size: u32) -> Vec<String> {
    serde_json::from_str(&js_snapshot_views(container_id, size)).unwrap_or_default()
}

pub fn stats(container_id: &str) -> ViewerStats {
    parse_stats(&js_stats(container_id))
}

fn parse_stats(json: &str) -> ViewerStats {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_json_is_parsed_and_garbage_falls_back_to_zeroes() {
        assert_eq!(
            parse_stats(r#"{"fps":60,"loadMs":412,"triangles":327680}"#),
            ViewerStats {
                fps: 60,
                load_ms: 412,
                triangles: 327_680
            }
        );
        assert_eq!(parse_stats("{}"), ViewerStats::default());
        assert_eq!(parse_stats("not json"), ViewerStats::default());
    }

    #[test]
    fn a_pick_payload_from_the_bridge_is_parsed() {
        let p: Pick = serde_json::from_str(r#"{"point":[12.5,-3,16],"normal":[0,0,1]}"#).unwrap();
        assert_eq!(p.point, [12.5, -3.0, 16.0]);
        assert_eq!(p.normal, [0.0, 0.0, 1.0]);
    }
}
