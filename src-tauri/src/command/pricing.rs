//! 成本定价器(M4)的命令:打印机 / 耗材档案、成本默认值、每个项目的成本模型与打样记录。
//!
//! 这里没有任何计算——算钱的公式全在 `pp_common::cost`(纯函数,前端改参数时本地即时重算)。
//! 后端只做存取;保存成本模型时顺手按同一套公式算出单位成本、冗余存一份,列表和项目中枢不用再算。

use std::sync::Arc;

use pp_common::cost::{breakdown, CostDefaults, CostModel, CostParams, Material, PricingDetail, PricingSummary, PrintRun, Printer};
use tauri::State;

use crate::AppCtx;

fn db<T>(r: pp_db::DbResult<T>) -> Result<T, String> {
    r.map_err(|e| e.to_wire())
}

// ---------------------------------------------------------------- 档案

#[tauri::command(rename_all = "snake_case")]
pub async fn printer_list(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<Printer>, String> {
    db(ctx.db.list_printers())
}

/// 新建(`id` 为空)或更新。
#[tauri::command(rename_all = "snake_case")]
pub async fn printer_save(ctx: State<'_, Arc<AppCtx>>, printer: Printer) -> Result<Printer, String> {
    db(ctx.db.save_printer(&printer))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn printer_delete(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    db(ctx.db.delete_printer(&id))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn material_list(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<Material>, String> {
    db(ctx.db.list_materials())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn material_save(ctx: State<'_, Arc<AppCtx>>, material: Material) -> Result<Material, String> {
    db(ctx.db.save_material(&material))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn material_delete(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    db(ctx.db.delete_material(&id))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cost_defaults_get(ctx: State<'_, Arc<AppCtx>>) -> Result<CostDefaults, String> {
    db(ctx.db.cost_defaults())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cost_defaults_set(ctx: State<'_, Arc<AppCtx>>, defaults: CostDefaults) -> Result<(), String> {
    db(ctx.db.set_cost_defaults(&defaults))
}

// ---------------------------------------------------------------- 项目的成本模型

/// 定价页打开一个项目:成本模型(没存过就是按默认值拼的草稿)+ 打样记录 + 调研给的价格带 + 名下模型的几何量。
#[tauri::command(rename_all = "snake_case")]
pub async fn pricing_get(ctx: State<'_, Arc<AppCtx>>, project_id: String) -> Result<PricingDetail, String> {
    Ok(PricingDetail {
        model: db(ctx.db.cost_model(&project_id))?,
        runs: db(ctx.db.list_print_runs(&project_id))?,
        price_band: db(ctx.db.project_price_band(&project_id))?,
        geometry: db(ctx.db.project_model_geometry(&project_id))?,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pricing_save(
    ctx: State<'_, Arc<AppCtx>>,
    project_id: String,
    params: CostParams,
    chosen_price: Option<f64>,
) -> Result<CostModel, String> {
    let unit_cost = breakdown(&params).unit_cost;
    db(ctx.db.save_cost_model(&project_id, &params, unit_cost, chosen_price))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pricing_summary(ctx: State<'_, Arc<AppCtx>>, project_id: String) -> Result<PricingSummary, String> {
    db(ctx.db.pricing_summary(&project_id))
}

// ---------------------------------------------------------------- 打样记录

#[tauri::command(rename_all = "snake_case")]
pub async fn print_run_add(ctx: State<'_, Arc<AppCtx>>, project_id: String, run: PrintRun) -> Result<PrintRun, String> {
    let saved = db(ctx.db.add_print_run(&project_id, &run))?;
    log::info!(
        "[pricing] 打样记录:{} · {:?} g · {:?} h",
        if saved.success { "成功" } else { "失败" },
        saved.actual_grams,
        saved.actual_hours
    );
    Ok(saved)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn print_run_delete(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    db(ctx.db.delete_print_run(&id))
}
