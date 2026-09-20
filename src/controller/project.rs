use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::td_string;
use pp_common::{NewProject, Project, ProjectStatus, Stage};

use crate::i18n_util::current_locale;
use crate::ipc::{self, cmd};
use crate::state::AppState;

/// 一次「跳过了中间阶段」的拖拽,等用户补一句原因。
#[derive(Clone, Debug, PartialEq)]
pub struct ForcedMove {
    pub project_id: String,
    pub code: String,
    pub from: Stage,
    pub to: Stage,
}

/// 往前跳过至少一个阶段才需要原因;往回拖(返工)和顺着走一步都不需要。
/// 阶段门清单做出来之后(MVP-α 第 2 周),这里换成「清单未满足」的判断。
pub fn needs_reason(from: Stage, to: Stage) -> bool {
    to.index() > from.index() + 1
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
        if needs_reason(p.stage, to) {
            prompt.set(Some(ForcedMove {
                project_id,
                code: p.code,
                from: p.stage,
                to,
            }));
        } else {
            self.move_stage(project_id, to, false, String::new());
        }
    }

    pub fn move_stage(self, project_id: String, to: Stage, forced: bool, note: String) {
        let state = self.state;
        // 乐观更新:卡片立刻落到新的一列,后端失败再整体重载
        state.projects.update(|list| {
            if let Some(p) = list.iter_mut().find(|p| p.id == project_id) {
                p.stage = to;
                p.stage_entered_at = crate::utils::now_ms();
            }
        });
        spawn_local(async move {
            let args = serde_json::json!({ "id": project_id, "to_stage": to, "forced": forced, "note": note });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_forward_skips_need_a_reason() {
        assert!(!needs_reason(Stage::Idea, Stage::Research), "顺着走一步");
        assert!(needs_reason(Stage::Idea, Stage::Concept), "跳过调研");
        assert!(needs_reason(Stage::Research, Stage::Listing));
        assert!(!needs_reason(Stage::Listing, Stage::Model), "往回拖 = 返工,不需要解释");
        assert!(!needs_reason(Stage::Model, Stage::Model));
    }
}
