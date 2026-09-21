use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::td_string;
use pp_common::design::CadDesign;
use pp_common::gate::{needs_reason, unmet_on_the_way, GateKey};
use pp_common::imagery::{ImageBoard, ImagePurpose};
use pp_common::{NewProject, Project, ProjectStatus, Stage};

use crate::i18n_util::current_locale;
use crate::ipc::{self, cmd};
use crate::state::{AppState, Handoff, Route};

/// 一次「证据还不够」的阶段推进,等用户补一句原因(规则在 pp_common::gate)。
#[derive(Clone, Debug, PartialEq)]
pub struct ForcedMove {
    pub project_id: String,
    pub code: String,
    pub from: Stage,
    pub to: Stage,
    /// 路过的阶段里还没完成的清单项;空 = 整个跳过了一个没法自动判断的阶段
    pub unmet: Vec<(Stage, GateKey)>,
}

/// 从项目里发起调研 / 出图时,输入框里先放的那句话:名称 + 品类 + 假设,够 AI 起步,用户可以改。
pub fn project_brief(p: &Project) -> String {
    let mut out = p.title.trim().to_string();
    if !p.category.trim().is_empty() {
        out.push_str(&format!("({})", p.category.trim()));
    }
    if !p.hypothesis.trim().is_empty() {
        out.push_str(&format!("。{}", p.hypothesis.trim()));
    }
    out
}

#[derive(Copy, Clone)]
pub struct ProjectController {
    state: AppState,
}

impl ProjectController {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    pub fn load(self) {
        let state = self.state;
        spawn_local(async move {
            match ipc::call_no_args::<Vec<Project>>(cmd::LIST_PROJECTS).await {
                Ok(list) => {
                    state.projects.set(list);
                    state.projects_loaded.set(true);
                }
                Err(e) => state.notify_error(e),
            }
        });
        state.reload_project_facts();
    }

    fn replace(self, updated: Project) {
        self.state.projects.update(|list| {
            match list.iter_mut().find(|p| p.id == updated.id) {
                Some(slot) => *slot = updated,
                None => list.insert(0, updated),
            }
        });
    }

    pub fn create(self, new: NewProject, on_done: impl Fn() + 'static) {
        let state = self.state;
        spawn_local(async move {
            let args = serde_json::json!({ "new": new });
            match ipc::call::<_, Project>(cmd::CREATE_PROJECT, &args).await {
                Ok(p) => {
                    state.notify_info(td_string!(current_locale(), project.created, code = &p.code).to_string());
                    self.replace(p);
                    on_done();
                }
                Err(e) => state.notify_error(e),
            }
        });
    }

    /// 看板拖拽的入口:需要原因的先交给 `prompt` 弹框,其余直接移动。
    pub fn request_move(self, project_id: String, to: Stage, prompt: RwSignal<Option<ForcedMove>>) {
        let Some(p) = self
            .state
            .projects
            .with_untracked(|list| list.iter().find(|p| p.id == project_id).cloned())
        else {
            return;
        };
        if p.stage == to {
            return;
        }
        let facts = self.state.project_facts.with_untracked(|m| m.get(&project_id).copied().unwrap_or_default());
        if needs_reason(p.stage, to, &facts) {
            prompt.set(Some(ForcedMove {
                unmet: unmet_on_the_way(p.stage, to, &facts),
                project_id,
                code: p.code,
                from: p.stage,
                to,
            }));
        } else {
            self.move_stage(project_id, to, String::new());
        }
    }

    /// 要不要写原因由后端按证据再判一次;这里只管把原因带过去。
    pub fn move_stage(self, project_id: String, to: Stage, note: String) {
        let state = self.state;
        // 乐观更新:卡片立刻落到新的一列,后端失败再整体重载
        state.projects.update(|list| {
            if let Some(p) = list.iter_mut().find(|p| p.id == project_id) {
                p.stage = to;
                p.stage_entered_at = crate::utils::now_ms();
            }
        });
        spawn_local(async move {
            let args = serde_json::json!({ "id": project_id, "to_stage": to, "note": note });
            match ipc::call::<_, Project>(cmd::MOVE_PROJECT_STAGE, &args).await {
                Ok(p) => self.replace(p),
                Err(e) => {
                    state.notify_error(e);
                    self.load();
                }
            }
        });
    }

    pub fn update(self, id: String, title: String, category: String, hypothesis: String) {
        let state = self.state;
        spawn_local(async move {
            let args = serde_json::json!({ "id": id, "title": title, "category": category, "hypothesis": hypothesis });
            match ipc::call::<_, Project>(cmd::UPDATE_PROJECT, &args).await {
                Ok(p) => {
                    self.replace(p);
                    // 假设写没写是「想法」阶段的清单项
                    state.reload_project_facts();
                    state.notify_info(td_string!(current_locale(), project.saved));
                }
                Err(e) => state.notify_error(e),
            }
        });
    }

    pub fn set_status(self, id: String, status: ProjectStatus, reason: Option<String>, on_done: impl Fn() + 'static) {
        let state = self.state;
        spawn_local(async move {
            let args = serde_json::json!({ "id": id, "status": status, "reason": reason });
            match ipc::call::<_, Project>(cmd::SET_PROJECT_STATUS, &args).await {
                Ok(p) => {
                    self.replace(p);
                    on_done();
                }
                Err(e) => state.notify_error(e),
            }
        });
    }

    pub fn delete(self, id: String, code: String) {
        let state = self.state;
        spawn_local(async move {
            match ipc::call_unit(cmd::DELETE_PROJECT, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    state.projects.update(|list| list.retain(|p| p.id != id));
                    state.open_project.set(None);
                    state.notify_info(td_string!(current_locale(), project.deleted, code = &code).to_string());
                }
                Err(e) => state.notify_error(e),
            }
        });
    }
}

// ---------------------------------------------------------------- 从项目里发起工作台的活
//
// 三个工作台(调研 / 图片 / 建模)都能独立用;从项目里发起时,产出直接挂在项目名下,
// 输入框里先放一句由项目名称、品类、假设拼出来的草稿。

impl ProjectController {
    fn close_and_go(self, handoff: Handoff, route: Route) {
        self.state.handoff.set(Some(handoff));
        self.state.open_project.set(None);
        self.state.go(route);
    }

    /// 为这个项目做一次调研。
    pub fn start_research(self, p: &Project) {
        let topic = if p.category.trim().is_empty() {
            p.title.trim().to_string()
        } else {
            format!("{} {}", p.category.trim(), p.title.trim())
        };
        self.close_and_go(
            Handoff::Research {
                project_id: p.id.clone(),
                topic,
            },
            Route::Ideas,
        );
    }

    pub fn open_research(self, run_id: String) {
        self.close_and_go(Handoff::ResearchRun { run_id }, Route::Ideas);
    }

    /// 为这个项目新建一个画板(建模参考图),挂在项目名下,跳到「图片」。
    pub fn start_board(self, p: &Project) {
        let (project_id, name, draft) = (p.id.clone(), p.title.clone(), project_brief(p));
        spawn_local(async move {
            let args = serde_json::json!({ "purpose": ImagePurpose::ModelRef, "project_id": project_id, "name": name });
            match ipc::call::<_, ImageBoard>(cmd::BOARD_CREATE, &args).await {
                Ok(b) => {
                    self.state.reload_project_facts();
                    self.close_and_go(Handoff::Board { board_id: b.id, draft }, Route::Images);
                }
                Err(e) => self.state.notify_error(e),
            }
        });
    }

    pub fn open_board(self, board_id: String) {
        self.close_and_go(
            Handoff::Board {
                board_id,
                draft: String::new(),
            },
            Route::Images,
        );
    }

    /// 为这个项目开始建模:有采用的预览图就拿它当参考图(`from_image` = 图的版本编号),没有就开一个空白设计。
    pub fn start_design(self, p: &Project, from_image: Option<String>) {
        let (project_id, name) = (p.id.clone(), p.title.clone());
        spawn_local(async move {
            let made = match from_image {
                Some(version_id) => ipc::call::<_, CadDesign>(cmd::BOARD_TO_DESIGN, &serde_json::json!({ "version_id": version_id })).await,
                None => ipc::call::<_, CadDesign>(cmd::DESIGN_CREATE, &serde_json::json!({ "name": name, "project_id": project_id })).await,
            };
            match made {
                Ok(d) => {
                    self.state.reload_project_facts();
                    self.close_and_go(
                        Handoff::Design {
                            design_id: d.id,
                            draft: String::new(),
                        },
                        Route::Studio,
                    );
                }
                Err(e) => self.state.notify_error(e),
            }
        });
    }

    /// 去「定价」:打样记录、成本模型、三档建议价。
    pub fn start_pricing(self, p: &Project) {
        self.close_and_go(Handoff::Pricing { project_id: p.id.clone() }, Route::Pricing);
    }

    /// 去「上架」:打开这个项目的草稿和发布包。
    pub fn start_publish(self, p: &Project) {
        self.close_and_go(Handoff::Publish { project_id: p.id.clone() }, Route::Publish);
    }

    pub fn open_design(self, design_id: String) {
        self.close_and_go(
            Handoff::Design {
                design_id,
                draft: String::new(),
            },
            Route::Studio,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brief_is_built_from_whatever_the_project_has() {
        let mut p = Project {
            id: "p".into(),
            code: "PP-0001".into(),
            title: " 磁吸线缆夹 ".into(),
            stage: Stage::Idea,
            status: ProjectStatus::Active,
            category: String::new(),
            hypothesis: String::new(),
            kill_reason: None,
            cover_asset_id: None,
            stage_entered_at: 0,
            created_at: 0,
            updated_at: 0,
        };
        assert_eq!(project_brief(&p), "磁吸线缆夹");
        p.category = "桌面收纳".into();
        p.hypothesis = "桌面党愿意为理线付 19 元".into();
        assert_eq!(project_brief(&p), "磁吸线缆夹(桌面收纳)。桌面党愿意为理线付 19 元");
    }
}
