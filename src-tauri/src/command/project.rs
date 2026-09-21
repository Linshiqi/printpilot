use std::collections::HashMap;
use std::sync::Arc;

use pp_common::gate::{needs_reason, ProjectFacts, ProjectOverview};
use pp_common::{NewProject, Project, ProjectStatus, Stage, StageEvent};
use tauri::State;

use super::cad::invalid;
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

/// 看板拖拽 / 「进入下一阶段」。
/// **要不要写原因由后端按证据判断**(pp_common::gate):路过的阶段清单没完成、或者整个跳过了一个没法自动判断的阶段,
/// 就必须带一句原因,这次流转记为「强行推进」。前端会先问,这里是兜底。
#[tauri::command(rename_all = "snake_case")]
pub async fn move_project_stage(ctx: State<'_, Arc<AppCtx>>, id: String, to_stage: Stage, note: String) -> Result<Project, String> {
    let current = ctx.db.get_project(&id).map_err(|e| e.to_wire())?;
    let facts = ctx.db.project_facts(&id).map_err(|e| e.to_wire())?;
    let forced = needs_reason(current.stage, to_stage, &facts);
    if forced && note.trim().is_empty() {
        return Err(invalid("this move skips unfinished stage work; a reason is required"));
    }
    let p = ctx
        .db
        .move_project_stage(&id, to_stage, "user", forced, &note)
        .map_err(|e| e.to_wire())?;
    log::info!("[project] {} → {}{}", p.code, to_stage.as_str(), if forced { "(强行)" } else { "" });
    Ok(p)
}

/// 所有项目名下的产出计数(看板卡片上的小图标、拖拽时判断要不要写原因)。
#[tauri::command(rename_all = "snake_case")]
pub async fn project_facts_all(ctx: State<'_, Arc<AppCtx>>) -> Result<HashMap<String, ProjectFacts>, String> {
    ctx.db.all_project_facts().map_err(|e| e.to_wire())
}

/// 项目中枢:这个项目名下的调研、画板、图、设计,和到现在的花费。
#[tauri::command(rename_all = "snake_case")]
pub async fn project_overview(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<ProjectOverview, String> {
    ctx.db.project_overview(&id).map_err(|e| e.to_wire())
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
