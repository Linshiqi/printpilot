use std::sync::Arc;

use pp_common::{NewProject, Project, ProjectStatus, Stage, StageEvent};
use tauri::State;

use crate::AppCtx;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_projects(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<Project>, String> {
    ctx.db.list_projects().map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_project(ctx: State<'_, Arc<AppCtx>>, new: NewProject) -> Result<Project, String> {
    let p = ctx.db.create_project(&new).map_err(|e| e.to_wire())?;
    log::info!("[project] 新建 {} {}", p.code, p.title);
    Ok(p)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_project(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<Project, String> {
    ctx.db.get_project(&id).map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_project(
    ctx: State<'_, Arc<AppCtx>>,
    id: String,
    title: String,
    category: String,
    hypothesis: String,
) -> Result<Project, String> {
    ctx.db
        .update_project(&id, &title, &category, &hypothesis)
        .map_err(|e| e.to_wire())
}

/// 看板拖拽 / 「进入下一阶段」。`forced` = 阶段门未满足时强行推进,必须带原因。
#[tauri::command(rename_all = "snake_case")]
pub async fn move_project_stage(
    ctx: State<'_, Arc<AppCtx>>,
    id: String,
    to_stage: Stage,
    forced: bool,
    note: String,
) -> Result<Project, String> {
    let p = ctx
        .db
        .move_project_stage(&id, to_stage, "user", forced, &note)
        .map_err(|e| e.to_wire())?;
    log::info!("[project] {} → {}{}", p.code, to_stage.as_str(), if forced { "(强行)" } else { "" });
    Ok(p)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_project_status(
    ctx: State<'_, Arc<AppCtx>>,
    id: String,
    status: ProjectStatus,
    reason: Option<String>,
) -> Result<Project, String> {
    ctx.db
        .set_project_status(&id, status, reason.as_deref())
        .map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_project(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    ctx.db.delete_project(&id).map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_stage_events(ctx: State<'_, Arc<AppCtx>>, project_id: String) -> Result<Vec<StageEvent>, String> {
    ctx.db.list_stage_events(&project_id).map_err(|e| e.to_wire())
}
