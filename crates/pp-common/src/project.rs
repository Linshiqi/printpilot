use serde::{Deserialize, Serialize};

use crate::stage::{ProjectStatus, Stage};

/// 一个「项目」= 一个单品的全生命周期。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    /// 人读的编号,如 PP-0012
    pub code: String,
    pub title: String,
    pub stage: Stage,
    pub status: ProjectStatus,
    #[serde(default)]
    pub category: String,
    /// 这个单品要验证的假设(「谁、为什么、愿意付多少」)
    #[serde(default)]
    pub hypothesis: String,
    #[serde(default)]
    pub kill_reason: Option<String>,
    #[serde(default)]
    pub cover_asset_id: Option<String>,
    /// 进入当前阶段的时间(毫秒),用来算「已停留 N 天」
    pub stage_entered_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Project {
    /// 在当前阶段已停留的整天数。
    pub fn days_in_stage(&self, now_ms: i64) -> u32 {
        let ms = (now_ms - self.stage_entered_at).max(0);
        (ms / 86_400_000) as u32
    }

    /// 是否该在看板上高亮「卡住了」。只对进行中的项目提醒。
    pub fn is_stale(&self, now_ms: i64) -> bool {
        self.status == ProjectStatus::Active
            && self
                .stage
                .stale_after_days()
                .is_some_and(|limit| self.days_in_stage(now_ms) >= limit)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewProject {
    pub title: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub hypothesis: String,
}

/// 阶段流转记录:用来算各阶段耗时与「想法→上架」周期。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageEvent {
    pub id: String,
    pub project_id: String,
    pub from_stage: Option<Stage>,
    pub to_stage: Stage,
    /// "user" | "agent"
    pub actor: String,
    /// 阶段门未满足时强行推进
    pub forced: bool,
    #[serde(default)]
    pub note: String,
    pub created_at: i64,
}

/// 「关于 / 设置」页展示的运行环境信息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub library_dir: String,
    pub db_path: String,
    pub schema_version: u32,
    pub project_count: u32,
    /// 演示模式:所有供应商适配器返回内置样例
    #[serde(default)]
    pub demo_mode: bool,
    /// 配置文件启动时读取失败则为 false:本次运行不保存设置,免得盖掉用户的配置
    #[serde(default)]
    pub config_writable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(stage: Stage, status: ProjectStatus, entered: i64) -> Project {
        Project {
            id: "p".into(),
            code: "PP-0001".into(),
            title: "t".into(),
            stage,
            status,
            category: String::new(),
            hypothesis: String::new(),
            kill_reason: None,
            cover_asset_id: None,
            stage_entered_at: entered,
            created_at: entered,
            updated_at: entered,
        }
    }

    const DAY: i64 = 86_400_000;

    #[test]
    fn days_in_stage_floors_and_never_goes_negative() {
        let p = project(Stage::Research, ProjectStatus::Active, 10 * DAY);
        assert_eq!(p.days_in_stage(10 * DAY + DAY - 1), 0);
        assert_eq!(p.days_in_stage(12 * DAY), 2);
        // 系统时钟回拨不应该下溢
        assert_eq!(p.days_in_stage(0), 0);
    }

    #[test]
    fn stale_only_for_active_projects_past_the_limit() {
        let p = project(Stage::Research, ProjectStatus::Active, 0);
        assert!(!p.is_stale(DAY));
        assert!(p.is_stale(2 * DAY));
        let paused = project(Stage::Research, ProjectStatus::Paused, 0);
        assert!(!paused.is_stale(30 * DAY));
        // 运营是长期阶段,不提醒
        let op = project(Stage::Operating, ProjectStatus::Active, 0);
        assert!(!op.is_stale(365 * DAY));
    }
}
