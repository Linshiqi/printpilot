//! 调研结果的持久化:一次调研 = `research_runs` 一行 + `opportunities` 若干行 + 费用账本一行。

use pp_common::research::{
    Evidence, Opportunity, ResearchBrief, ResearchReport, ResearchRunSummary, ResearchUsage, SavedResearch,
};
use pp_common::{NewProject, Project, Stage};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::projects::insert_project;
use crate::{new_id, now_ms, Db, DbError, DbResult};

/// `research_runs.sources_json` 里放的东西:正文与机会各有自己的列,其余的都在这里。
#[derive(Serialize, Deserialize)]
struct RunExtras {
    evidence: Vec<Evidence>,
    usage: ResearchUsage,
    degraded: bool,
    reviewed: bool,
}

fn json_err(e: serde_json::Error) -> DbError {
    DbError::Invalid(format!("research json: {e}"))
}

impl Db {
    /// 保存一次调研,返回 run id。花费同时记进费用账本(不足 1 分按 1 分记,宁多勿少)。
    pub fn save_research(&self, project_id: Option<&str>, brief: &ResearchBrief, report: &ResearchReport) -> DbResult<String> {
        let run_id = new_id();
        let at = now_ms();
        let cost_fen = report.usage.cost_fen.ceil().max(0.0) as i64;
        let extras = RunExtras {
            evidence: report.evidence.clone(),
            usage: report.usage,
            degraded: report.degraded,
            reviewed: report.reviewed,
        };

        let mut conn = self.w();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO research_runs(id, project_id, query, params_json, status, report_md, sources_json,
                                       cost, created_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, 'succeeded', ?5, ?6, ?7, ?8, ?8)",
            params![
                run_id,
                project_id,
                brief.topic.trim(),
                serde_json::to_string(brief).map_err(json_err)?,
                report.summary_md,
                serde_json::to_string(&extras).map_err(json_err)?,
                cost_fen,
                at
            ],
        )?;
        for o in &report.opportunities {
            tx.execute(
                "INSERT INTO opportunities(id, research_run_id, title, summary, scores_json, total_score,
                                           detail_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_id(),
                    run_id,
                    o.title,
                    o.pitch,
                    serde_json::to_string(&o.scores).map_err(json_err)?,
                    o.total,
                    serde_json::to_string(o).map_err(json_err)?,
                    at
                ],
            )?;
        }
        if cost_fen > 0 {
            tx.execute(
                "INSERT INTO cost_entries(id, project_id, job_id, category, amount, note, created_at)
                 VALUES (?1, ?2, NULL, 'research', ?3, ?4, ?5)",
                params![new_id(), project_id, cost_fen, brief.topic.trim(), at],
            )?;
        }
        tx.commit()?;
        Ok(run_id)
    }

    pub fn list_research_runs(&self, limit: u32) -> DbResult<Vec<ResearchRunSummary>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT r.id, r.query, r.created_at, r.cost,
                    (SELECT COUNT(*) FROM opportunities o WHERE o.research_run_id = r.id)
             FROM research_runs r ORDER BY r.created_at DESC, r.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            Ok(ResearchRunSummary {
                id: r.get(0)?,
                topic: r.get(1)?,
                created_at: r.get(2)?,
                cost_fen: r.get(3)?,
                opportunities: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_research(&self, run_id: &str) -> DbResult<SavedResearch> {
        let conn = self.r();
        let (brief_json, summary_md, extras_json): (String, String, String) = conn
            .query_row(
                "SELECT params_json, report_md, sources_json FROM research_runs WHERE id = ?1",
                [run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("research run {run_id}")))?;
        let brief: ResearchBrief = serde_json::from_str(&brief_json).map_err(json_err)?;
        let extras: RunExtras = serde_json::from_str(&extras_json).map_err(json_err)?;

        // 插入顺序 = 流水线排好的顺序(总分从高到低,被否决的垫底)
        let mut stmt = conn.prepare("SELECT detail_json FROM opportunities WHERE research_run_id = ?1 ORDER BY rowid")?;
        let details: Vec<String> = stmt
            .query_map([run_id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let opportunities = details
            .iter()
            .map(|d| serde_json::from_str::<Opportunity>(d).map_err(json_err))
            .collect::<DbResult<Vec<_>>>()?;

        Ok(SavedResearch {
            run_id: run_id.to_string(),
            brief,
            report: ResearchReport {
                summary_md,
                opportunities,
                evidence: extras.evidence,
                usage: extras.usage,
                degraded: extras.degraded,
                reviewed: extras.reviewed,
            },
        })
    }

    /// 「采用为项目」:用机会卡的内容建一个项目(直接进入「调研」阶段——调研已经做完了),
    /// 并把这张机会卡标记为已采用。IP 风险为「高」的机会不能采用(一票否决)。
    pub fn adopt_opportunity(&self, run_id: &str, index: usize) -> DbResult<Project> {
        let saved = self.get_research(run_id)?;
        let opp = saved
            .report
            .opportunities
            .get(index)
            .ok_or_else(|| DbError::NotFound(format!("opportunity #{index} of run {run_id}")))?;
        if opp.vetoed() {
            return Err(DbError::Invalid("opportunity is vetoed (high IP risk)".into()));
        }
        let new = NewProject {
            title: opp.title.clone(),
            category: saved.brief.topic.trim().to_string(),
            hypothesis: format!("{}。{}。预期价格带 ¥{}~{}。", opp.persona, opp.pitch, opp.price_low, opp.price_high),
        };
        let project_id = {
            let mut conn = self.w();
            let tx = conn.transaction()?;
            let row_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM opportunities WHERE research_run_id = ?1 ORDER BY rowid LIMIT 1 OFFSET ?2",
                    params![run_id, index as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let project_id = insert_project(&tx, &new, Stage::Research, "agent", &format!("采用自调研「{}」", saved.brief.topic.trim()))?;
            if let Some(row_id) = row_id {
                tx.execute(
                    "UPDATE opportunities SET adopted_project_id = ?2 WHERE id = ?1",
                    params![row_id, project_id],
                )?;
            }
            tx.execute(
                "UPDATE research_runs SET project_id = COALESCE(project_id, ?2) WHERE id = ?1",
                params![run_id, project_id],
            )?;
            tx.commit()?;
            project_id
        };
        self.get_project(&project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;
    use pp_common::research::{EvidenceGrade, IpRisk, ModelRoute, Scores};

    fn opp(title: &str, ip: IpRisk, total: f64) -> Opportunity {
        Opportunity {
            title: title.into(),
            pitch: "磁吸 + 好看".into(),
            persona: "桌搭人群".into(),
            selling_points: vec!["磁吸".into()],
            price_low: 19,
            price_high: 39,
            scores: Scores {
                demand: 4.0,
                ..Default::default()
            },
            ip_risk: ip,
            rationale: "见 [1]".into(),
            evidence_ids: vec![1],
            risks: vec![],
            route: ModelRoute::Parametric,
            size_mm: 60.0,
            grams: 25.0,
            total,
        }
    }

    fn sample() -> (ResearchBrief, ResearchReport) {
        let brief: ResearchBrief = serde_json::from_str(r#"{ "topic": " 桌面收纳 " }"#).unwrap();
        let report = ResearchReport {
            summary_md: "需求稳定 [1]".into(),
            opportunities: vec![opp("磁吸线缆夹", IpRisk::Low, 3.9), opp("某动漫支架", IpRisk::High, 4.5)],
            evidence: vec![Evidence {
                id: 1,
                title: "t".into(),
                url: "https://example.com".into(),
                site: "示例".into(),
                published: None,
                excerpt: "e".into(),
                grade: EvidenceGrade::B,
            }],
            usage: ResearchUsage {
                llm_calls: 3,
                searches: 3,
                cost_fen: 27.2,
                ..Default::default()
            },
            degraded: false,
            reviewed: true,
        };
        (brief, report)
    }

    #[test]
    fn a_saved_run_reads_back_identically_and_bills_the_ledger() {
        let t = TempDb::new();
        let (brief, report) = sample();
        let run_id = t.save_research(None, &brief, &report).unwrap();

        let back = t.get_research(&run_id).unwrap();
        assert_eq!(back.brief, brief);
        assert_eq!(back.report, report, "含机会的顺序");

        let runs = t.list_research_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].topic.as_str(), runs[0].cost_fen, runs[0].opportunities), ("桌面收纳", 28, 2));

        let billed: i64 = t
            .r()
            .query_row("SELECT SUM(amount) FROM cost_entries WHERE category = 'research'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(billed, 28, "27.2 分向上取整");
    }

    #[test]
    fn adopting_creates_a_project_in_the_research_stage_and_marks_the_card() {
        let t = TempDb::new();
        let (brief, report) = sample();
        let run_id = t.save_research(None, &brief, &report).unwrap();

        let p = t.adopt_opportunity(&run_id, 0).unwrap();
        assert_eq!(p.title, "磁吸线缆夹");
        assert_eq!(p.category, "桌面收纳");
        assert_eq!(p.stage, Stage::Research);
        assert!(p.hypothesis.contains("¥19~39"));
        let events = t.list_stage_events(&p.id).unwrap();
        assert_eq!(events[0].actor, "agent");
        assert!(events[0].note.contains("桌面收纳"));

        let adopted: Option<String> = t
            .r()
            .query_row(
                "SELECT adopted_project_id FROM opportunities WHERE research_run_id = ?1 ORDER BY rowid LIMIT 1",
                [&run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(adopted.as_deref(), Some(p.id.as_str()));
    }

    #[test]
    fn vetoed_or_missing_opportunities_cannot_be_adopted() {
        let t = TempDb::new();
        let (brief, report) = sample();
        let run_id = t.save_research(None, &brief, &report).unwrap();
        assert!(matches!(t.adopt_opportunity(&run_id, 1), Err(DbError::Invalid(_))), "IP 风险高 = 一票否决");
        assert!(matches!(t.adopt_opportunity(&run_id, 9), Err(DbError::NotFound(_))));
        assert!(matches!(t.adopt_opportunity("nope", 0), Err(DbError::NotFound(_))));
        assert_eq!(t.project_count().unwrap(), 0, "失败的采用不能留下半个项目");
    }

    #[test]
    fn free_runs_do_not_clutter_the_ledger() {
        let t = TempDb::new();
        let (brief, mut report) = sample();
        report.usage.cost_fen = 0.0; // 演示模式
        t.save_research(None, &brief, &report).unwrap();
        let rows: i64 = t.r().query_row("SELECT COUNT(*) FROM cost_entries", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0);
    }
}
