//! 费用账本。金额单位:分(整数)。模型调用按 token 算出来的是小数——**向上取整**,宁多勿少。

use rusqlite::params;

use crate::{new_id, now_ms, Db, DbResult};

impl Db {
    /// 记一笔。`cost_fen <= 0`(演示模式、全部命中缓存…)不记,别让零元条目把账本塞满。
    /// `category`:llm / search / image / model3d / research …(docs/03-architecture.md §9)
    pub fn add_cost(&self, project_id: Option<&str>, category: &str, cost_fen: f64, note: &str) -> DbResult<i64> {
        if !(cost_fen.is_finite() && cost_fen > 0.0) {
            return Ok(0);
        }
        let amount = cost_fen.ceil() as i64;
        self.w().execute(
            "INSERT INTO cost_entries(id, project_id, job_id, category, amount, note, created_at)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)",
            params![new_id(), project_id, category, amount, note, now_ms()],
        )?;
        Ok(amount)
    }

    /// 某一类费用的累计(分)。
    pub fn total_cost(&self, category: &str) -> DbResult<i64> {
        Ok(self.r().query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM cost_entries WHERE category = ?1",
            [category],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::TempDb;

    #[test]
    fn fractions_round_up_and_free_calls_leave_no_trace() {
        let t = TempDb::new();
        assert_eq!(t.add_cost(None, "model3d", 12.3, "线缆夹 · 生成").unwrap(), 13);
        assert_eq!(t.add_cost(None, "model3d", 0.2, "线缆夹 · 复核").unwrap(), 1);
        assert_eq!(t.add_cost(None, "model3d", 0.0, "演示").unwrap(), 0);
        assert_eq!(t.add_cost(None, "model3d", f64::NAN, "坏数据").unwrap(), 0);
        assert_eq!(t.total_cost("model3d").unwrap(), 14);
        assert_eq!(t.total_cost("research").unwrap(), 0);
    }
}
