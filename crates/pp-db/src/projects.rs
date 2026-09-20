use pp_common::{NewProject, Project, ProjectStatus, Stage, StageEvent};
use rusqlite::{params, OptionalExtension, Row, Transaction};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const COLS: &str = "id, code, title, stage, status, category, hypothesis, kill_reason, \
                    cover_asset_id, stage_entered_at, created_at, updated_at";

fn row_to_project(r: &Row<'_>) -> rusqlite::Result<Project> {
    let stage: String = r.get(3)?;
    let status: String = r.get(4)?;
    Ok(Project {
        id: r.get(0)?,
        code: r.get(1)?,
        title: r.get(2)?,
        // 未知取值(更新的版本写进来的)降级到最保守的那一项,而不是让整个列表读不出来
        stage: Stage::parse(&stage).unwrap_or_else(|| {
            log::warn!("[db] 未知阶段 {stage:?},按 idea 处理");
            Stage::Idea
        }),
        status: ProjectStatus::parse(&status).unwrap_or_else(|| {
            log::warn!("[db] 未知状态 {status:?},按 paused 处理");
            ProjectStatus::Paused
        }),
        category: r.get(5)?,
        hypothesis: r.get(6)?,
        kill_reason: r.get(7)?,
        cover_asset_id: r.get(8)?,
        stage_entered_at: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
    })
}

fn next_project_code(tx: &Transaction<'_>) -> rusqlite::Result<String> {
    let cur: i64 = tx
        .query_row(
            "SELECT value FROM meta WHERE key = 'project_seq'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let next = cur + 1;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('project_seq', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [next.to_string()],
    )?;
    Ok(format!("PP-{next:04}"))
}

fn insert_stage_event(
    tx: &Transaction<'_>,
    project_id: &str,
    from: Option<Stage>,
    to: Stage,
    actor: &str,
    forced: bool,
    note: &str,
    at: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO stage_events(id, project_id, from_stage, to_stage, actor, forced, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            new_id(),
            project_id,
            from.map(Stage::as_str),
            to.as_str(),
            actor,
            forced,
            note,
            at
        ],
    )?;
    Ok(())
}

/// 在一个已有事务里建项目(「采用调研机会」要和标记机会已采用放在同一个事务里)。
pub(crate) fn insert_project(
    tx: &Transaction<'_>,
    new: &NewProject,
    stage: Stage,
    actor: &str,
    note: &str,
) -> DbResult<String> {
    let title = new.title.trim();
    if title.is_empty() {
        return Err(DbError::Invalid("title is empty".into()));
    }
    let id = new_id();
    let at = now_ms();
    let code = next_project_code(tx)?;
    tx.execute(
        "INSERT INTO projects(id, code, title, stage, status, category, hypothesis,
                              stage_entered_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?7, ?7)",
        params![
            id,
            code,
            title,
            stage.as_str(),
            new.category.trim(),
            new.hypothesis.trim(),
            at
        ],
    )?;
    insert_stage_event(tx, &id, None, stage, actor, false, note, at)?;
    Ok(id)
}

impl Db {
    pub fn create_project(&self, new: &NewProject) -> DbResult<Project> {
        let id = {
            let mut conn = self.w();
            let tx = conn.transaction()?;
            let id = insert_project(&tx, new, Stage::Idea, "user", "")?;
            tx.commit()?;
            id
        };
        self.get_project(&id)
    }

    pub fn get_project(&self, id: &str) -> DbResult<Project> {
        self.r()
            .query_row(
                &format!("SELECT {COLS} FROM projects WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                row_to_project,
            )
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("project {id}")))
    }

    /// 未删除的全部项目;最近进入当前阶段的排前面(看板每列的卡片顺序)。
    pub fn list_projects(&self) -> DbResult<Vec<Project>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM projects WHERE deleted_at IS NULL
             ORDER BY stage_entered_at DESC, created_at DESC"
        ))?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn project_count(&self) -> DbResult<u32> {
        Ok(self.r().query_row(
            "SELECT COUNT(*) FROM projects WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?)
    }

    /// 阶段流转。目标阶段与当前相同则什么都不做(拖回原列不应该刷一条流转记录)。
    /// `forced` = 阶段门未满足时强行推进,此时 `note` 记原因(docs/01-prd.md §4)。
    pub fn move_project_stage(
        &self,
        id: &str,
        to: Stage,
        actor: &str,
        forced: bool,
        note: &str,
    ) -> DbResult<Project> {
        let cur = self.get_project(id)?;
        if cur.stage == to {
            return Ok(cur);
        }
        if forced && note.trim().is_empty() {
            return Err(DbError::Invalid("forced move needs a reason".into()));
        }
        let at = now_ms();
        {
            let mut conn = self.w();
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE projects SET stage = ?2, stage_entered_at = ?3, updated_at = ?3 WHERE id = ?1",
                params![id, to.as_str(), at],
            )?;
            insert_stage_event(&tx, id, Some(cur.stage), to, actor, forced, note.trim(), at)?;
            tx.commit()?;
        }
        self.get_project(id)
    }

    /// 暂停 / 恢复 / 淘汰 / 完成。淘汰必须给原因——淘汰原因是复盘时最值钱的数据。
    pub fn set_project_status(
        &self,
        id: &str,
        status: ProjectStatus,
        reason: Option<&str>,
    ) -> DbResult<Project> {
        let reason = reason.map(str::trim).filter(|s| !s.is_empty());
        if status == ProjectStatus::Killed && reason.is_none() {
            return Err(DbError::Invalid("kill needs a reason".into()));
        }
        let kill_reason = (status == ProjectStatus::Killed).then_some(reason).flatten();
        let n = self.w().execute(
            "UPDATE projects SET status = ?2, kill_reason = ?3, updated_at = ?4
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id, status.as_str(), kill_reason, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("project {id}")));
        }
        self.get_project(id)
    }

    pub fn update_project(
        &self,
        id: &str,
        title: &str,
        category: &str,
        hypothesis: &str,
    ) -> DbResult<Project> {
        let title = title.trim();
        if title.is_empty() {
            return Err(DbError::Invalid("title is empty".into()));
        }
        let n = self.w().execute(
            "UPDATE projects SET title = ?2, category = ?3, hypothesis = ?4, updated_at = ?5
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id, title, category.trim(), hypothesis.trim(), now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("project {id}")));
        }
        self.get_project(id)
    }

    /// 软删除:流转记录与资产保留,将来可以做「回收站」。
    pub fn delete_project(&self, id: &str) -> DbResult<()> {
        let n = self.w().execute(
            "UPDATE projects SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("project {id}")));
        }
        Ok(())
    }

    pub fn list_stage_events(&self, project_id: &str) -> DbResult<Vec<StageEvent>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, from_stage, to_stage, actor, forced, note, created_at
             FROM stage_events WHERE project_id = ?1 ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            let from: Option<String> = r.get(2)?;
            let to: String = r.get(3)?;
            Ok(StageEvent {
                id: r.get(0)?,
                project_id: r.get(1)?,
                from_stage: from.as_deref().and_then(Stage::parse),
                to_stage: Stage::parse(&to).unwrap_or(Stage::Idea),
                actor: r.get(4)?,
                forced: r.get(5)?,
                note: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;

    fn new(title: &str) -> NewProject {
        NewProject {
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn codes_are_sequential_and_survive_deletion() {
        let t = TempDb::new();
        let a = t.create_project(&new("a")).unwrap();
        let b = t.create_project(&new("b")).unwrap();
        assert_eq!((a.code.as_str(), b.code.as_str()), ("PP-0001", "PP-0002"));
        // 删掉最新的,编号也不回收——编号会出现在导出的文件夹名和用户的笔记里
        t.delete_project(&b.id).unwrap();
        assert_eq!(t.create_project(&new("c")).unwrap().code, "PP-0003");
    }

    #[test]
    fn create_trims_and_rejects_blank_title() {
        let t = TempDb::new();
        assert!(matches!(
            t.create_project(&new("   ")),
            Err(DbError::Invalid(_))
        ));
        let p = t.create_project(&new("  线缆夹  ")).unwrap();
        assert_eq!(p.title, "线缆夹");
        assert_eq!(p.stage, Stage::Idea);
        assert_eq!(p.status, ProjectStatus::Active);
    }

    #[test]
    fn creation_logs_the_first_stage_event() {
        let t = TempDb::new();
        let p = t.create_project(&new("a")).unwrap();
        let ev = t.list_stage_events(&p.id).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].from_stage, ev[0].to_stage), (None, Stage::Idea));
    }

    #[test]
    fn moving_stage_updates_entry_time_and_logs_event() {
        let t = TempDb::new();
        let p = t.create_project(&new("a")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let moved = t
            .move_project_stage(&p.id, Stage::Concept, "user", false, "")
            .unwrap();
        assert_eq!(moved.stage, Stage::Concept);
        assert!(moved.stage_entered_at > p.stage_entered_at);
        let ev = t.list_stage_events(&p.id).unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(
            (ev[1].from_stage, ev[1].to_stage),
            (Some(Stage::Idea), Stage::Concept)
        );
    }

    #[test]
    fn dropping_a_card_back_on_its_own_column_is_a_noop() {
        let t = TempDb::new();
        let p = t.create_project(&new("a")).unwrap();
        let same = t
            .move_project_stage(&p.id, Stage::Idea, "user", false, "")
            .unwrap();
        assert_eq!(same.stage_entered_at, p.stage_entered_at);
        assert_eq!(t.list_stage_events(&p.id).unwrap().len(), 1);
    }

    #[test]
    fn forced_move_requires_a_reason_and_records_it() {
        let t = TempDb::new();
        let p = t.create_project(&new("a")).unwrap();
        assert!(matches!(
            t.move_project_stage(&p.id, Stage::Listing, "user", true, "  "),
            Err(DbError::Invalid(_))
        ));
        t.move_project_stage(&p.id, Stage::Listing, "user", true, "已有现成模型")
            .unwrap();
        let ev = t.list_stage_events(&p.id).unwrap();
        assert!(ev[1].forced);
        assert_eq!(ev[1].note, "已有现成模型");
    }

    #[test]
    fn killing_needs_a_reason_and_reviving_clears_it() {
        let t = TempDb::new();
        let p = t.create_project(&new("a")).unwrap();
        assert!(matches!(
            t.set_project_status(&p.id, ProjectStatus::Killed, None),
            Err(DbError::Invalid(_))
        ));
        let killed = t
            .set_project_status(&p.id, ProjectStatus::Killed, Some("测款收藏率 1.2%"))
            .unwrap();
        assert_eq!(killed.kill_reason.as_deref(), Some("测款收藏率 1.2%"));
        let back = t
            .set_project_status(&p.id, ProjectStatus::Active, None)
            .unwrap();
        assert_eq!(back.kill_reason, None);
    }

    #[test]
    fn deleted_projects_disappear_from_reads() {
        let t = TempDb::new();
        let a = t.create_project(&new("a")).unwrap();
        let b = t.create_project(&new("b")).unwrap();
        t.delete_project(&a.id).unwrap();
        let list = t.list_projects().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, b.id);
        assert_eq!(t.project_count().unwrap(), 1);
        assert!(matches!(t.get_project(&a.id), Err(DbError::NotFound(_))));
        assert!(matches!(t.delete_project(&a.id), Err(DbError::NotFound(_))));
    }

    #[test]
    fn unknown_ids_are_not_found() {
        let t = TempDb::new();
        assert!(matches!(
            t.move_project_stage("nope", Stage::Model, "user", false, ""),
            Err(DbError::NotFound(_))
        ));
        assert!(matches!(
            t.update_project("nope", "x", "", ""),
            Err(DbError::NotFound(_))
        ));
    }
}
