//! 成本定价器(M4)的仓储:打印机 / 耗材档案、打样记录、每个项目的成本模型、资料库级的成本默认值。
//! (`costs.rs` 是另一回事:那是 AI 接口的花费账本。)
//!
//! 金额:界面和计算用「元」的小数(pp_common::cost),这里落库时换成整数「分」。

use pp_common::cost::{CostDefaults, CostModel, CostParams, Material, ModelGeometry, PricingSummary, PrintRun, Printer};
use rusqlite::{params, OptionalExtension, Row};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const DEFAULTS_KEY: &str = "cost_defaults";

fn to_fen(yuan: f64) -> i64 {
    if yuan.is_finite() && yuan > 0.0 {
        (yuan * 100.0).round() as i64
    } else {
        0
    }
}

fn to_yuan(fen: i64) -> f64 {
    fen as f64 / 100.0
}

fn json_err(e: serde_json::Error) -> DbError {
    DbError::Invalid(format!("pricing json: {e}"))
}

fn named(name: &str, what: &str) -> DbResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DbError::Invalid(format!("{what} name is empty")));
    }
    Ok(name.to_string())
}

fn row_to_printer(r: &Row<'_>) -> rusqlite::Result<Printer> {
    Ok(Printer {
        id: r.get(0)?,
        name: r.get(1)?,
        model: r.get(2)?,
        build_mm: [r.get(3)?, r.get(4)?, r.get(5)?],
        power_w: r.get(6)?,
        price_yuan: to_yuan(r.get(7)?),
        lifetime_hours: r.get(8)?,
        grams_per_hour: r.get(9)?,
        created_at: r.get(10)?,
    })
}

fn row_to_material(r: &Row<'_>) -> rusqlite::Result<Material> {
    Ok(Material {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        color: r.get(3)?,
        yuan_per_kg: to_yuan(r.get(4)?),
        density: r.get(5)?,
        stock_g: r.get(6)?,
        created_at: r.get(7)?,
    })
}

fn row_to_run(r: &Row<'_>) -> rusqlite::Result<PrintRun> {
    let hours = |m: Option<f64>| m.map(|m| m / 60.0);
    let result: String = r.get(8)?;
    Ok(PrintRun {
        id: r.get(0)?,
        project_id: r.get(1)?,
        printer_id: r.get(2)?,
        material_id: r.get(3)?,
        est_hours: hours(r.get(4)?),
        est_grams: r.get(5)?,
        actual_hours: hours(r.get(6)?),
        actual_grams: r.get(7)?,
        success: result == "success",
        fail_reason: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        note: r.get(10)?,
        created_at: r.get(11)?,
    })
}

impl Db {
    // ---------------------------------------------------------------- 档案

    pub fn list_printers(&self) -> DbResult<Vec<Printer>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT id, name, model, build_x, build_y, build_z, power_w, price, lifetime_hours, grams_per_hour, created_at
             FROM printers WHERE deleted_at IS NULL ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], row_to_printer)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// 新建(`id` 为空)或更新。
    pub fn save_printer(&self, p: &Printer) -> DbResult<Printer> {
        let name = named(&p.name, "printer")?;
        let positive = [p.power_w, p.lifetime_hours, p.grams_per_hour].iter().chain(&p.build_mm).all(|v| v.is_finite() && *v > 0.0);
        if !positive {
            return Err(DbError::Invalid("printer numbers must be positive".into()));
        }
        let id = if p.id.is_empty() { new_id() } else { p.id.clone() };
        let n = self.w().execute(
            "INSERT INTO printers(id, name, model, build_x, build_y, build_z, power_w, price, lifetime_hours, grams_per_hour, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET name = ?2, model = ?3, build_x = ?4, build_y = ?5, build_z = ?6, power_w = ?7,
                                           price = ?8, lifetime_hours = ?9, grams_per_hour = ?10
             WHERE deleted_at IS NULL",
            params![id, name, p.model.trim(), p.build_mm[0], p.build_mm[1], p.build_mm[2], p.power_w, to_fen(p.price_yuan), p.lifetime_hours, p.grams_per_hour, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("printer {id}")));
        }
        self.list_printers()?.into_iter().find(|x| x.id == id).ok_or_else(|| DbError::NotFound(format!("printer {id}")))
    }

    pub fn delete_printer(&self, id: &str) -> DbResult<()> {
        let n = self
            .w()
            .execute("UPDATE printers SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, now_ms()])?;
        if n == 0 {
            return Err(DbError::NotFound(format!("printer {id}")));
        }
        Ok(())
    }

    pub fn list_materials(&self) -> DbResult<Vec<Material>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT id, name, type, color, cost_per_kg, density, stock_g, created_at
             FROM materials WHERE deleted_at IS NULL ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], row_to_material)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn save_material(&self, m: &Material) -> DbResult<Material> {
        let name = named(&m.name, "material")?;
        if !(m.yuan_per_kg.is_finite() && m.yuan_per_kg > 0.0 && m.density.is_finite() && m.density > 0.0) {
            return Err(DbError::Invalid("material price and density must be positive".into()));
        }
        let id = if m.id.is_empty() { new_id() } else { m.id.clone() };
        let kind = if m.kind.trim().is_empty() { "PLA" } else { m.kind.trim() };
        let n = self.w().execute(
            "INSERT INTO materials(id, name, type, color, cost_per_kg, density, stock_g, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET name = ?2, type = ?3, color = ?4, cost_per_kg = ?5, density = ?6, stock_g = ?7
             WHERE deleted_at IS NULL",
            params![id, name, kind, m.color.trim(), to_fen(m.yuan_per_kg), m.density, m.stock_g.max(0.0), now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("material {id}")));
        }
        self.list_materials()?.into_iter().find(|x| x.id == id).ok_or_else(|| DbError::NotFound(format!("material {id}")))
    }

    pub fn delete_material(&self, id: &str) -> DbResult<()> {
        let n = self
            .w()
            .execute("UPDATE materials SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, now_ms()])?;
        if n == 0 {
            return Err(DbError::NotFound(format!("material {id}")));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- 成本默认值

    /// 资料库里没存过就是出厂默认值。存坏了的(手改过库)也回到默认值,不让定价页打不开。
    pub fn cost_defaults(&self) -> DbResult<CostDefaults> {
        let json: Option<String> = self
            .r()
            .query_row("SELECT value_json FROM library_settings WHERE key = ?1", [DEFAULTS_KEY], |r| r.get(0))
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
    }

    pub fn set_cost_defaults(&self, d: &CostDefaults) -> DbResult<()> {
        let rates_ok = [d.waste_rate, d.success_rate, d.fee_rate].iter().chain(&d.target_margins).all(|v| v.is_finite() && (0.0..=1.0).contains(v));
        if !rates_ok || d.success_rate <= 0.0 {
            return Err(DbError::Invalid("rates must be between 0 and 1".into()));
        }
        let json = serde_json::to_string(d).map_err(json_err)?;
        self.w().execute(
            "INSERT INTO library_settings(key, value_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = ?2, updated_at = ?3",
            params![DEFAULTS_KEY, json, now_ms()],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- 成本模型

    /// 项目的成本模型;还没存过的,给一份按默认值拼出来的草稿(`saved = false`)。
    pub fn cost_model(&self, project_id: &str) -> DbResult<CostModel> {
        self.get_project(project_id)?;
        let row: Option<(String, Option<i64>, i64)> = self
            .r()
            .query_row("SELECT params_json, chosen_price, updated_at FROM cost_models WHERE project_id = ?1", [project_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        Ok(match row {
            Some((json, price, updated_at)) => CostModel {
                // 少字段的旧数据用默认值补(`#[serde(default)]`);整个坏了就回到默认
                params: serde_json::from_str(&json).unwrap_or_default(),
                chosen_price: price.map(to_yuan),
                saved: true,
                updated_at,
            },
            None => CostModel {
                params: self.cost_defaults()?.new_params(),
                chosen_price: None,
                saved: false,
                updated_at: 0,
            },
        })
    }

    /// `unit_cost` 由调用方按 pp_common::cost 算好传进来(冗余存一份,列表和项目中枢不用再算)。
    pub fn save_cost_model(&self, project_id: &str, params_in: &CostParams, unit_cost_yuan: f64, chosen_price: Option<f64>) -> DbResult<CostModel> {
        self.get_project(project_id)?;
        let json = serde_json::to_string(params_in).map_err(json_err)?;
        let price = chosen_price.filter(|p| p.is_finite() && *p > 0.0).map(to_fen);
        self.w().execute(
            "INSERT INTO cost_models(project_id, params_json, unit_cost, chosen_price, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(project_id) DO UPDATE SET params_json = ?2, unit_cost = ?3, chosen_price = ?4, updated_at = ?5",
            params![project_id, json, to_fen(unit_cost_yuan), price, now_ms()],
        )?;
        self.cost_model(project_id)
    }

    // ---------------------------------------------------------------- 打样记录

    /// 最新的在前。
    pub fn list_print_runs(&self, project_id: &str) -> DbResult<Vec<PrintRun>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, printer_id, material_id, est_minutes, est_grams, actual_minutes, actual_grams, result, fail_reason, note, created_at
             FROM print_runs WHERE project_id = ?1 AND deleted_at IS NULL ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([project_id], row_to_run)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_print_run(&self, project_id: &str, run: &PrintRun) -> DbResult<PrintRun> {
        self.get_project(project_id)?;
        if !run.success && run.fail_reason.trim().is_empty() {
            // 失败原因是以后改设计、调参数的依据,和淘汰原因一个道理
            return Err(DbError::Invalid("a failed print run needs a reason".into()));
        }
        let minutes = |h: Option<f64>| h.filter(|v| v.is_finite() && *v > 0.0).map(|v| v * 60.0);
        let grams = |g: Option<f64>| g.filter(|v| v.is_finite() && *v > 0.0);
        let id = new_id();
        self.w().execute(
            "INSERT INTO print_runs(id, project_id, printer_id, material_id, est_minutes, est_grams, actual_minutes, actual_grams,
                                    result, fail_reason, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                project_id,
                run.printer_id,
                run.material_id,
                minutes(run.est_hours),
                grams(run.est_grams),
                minutes(run.actual_hours),
                grams(run.actual_grams),
                if run.success { "success" } else { "failed" },
                (!run.success).then(|| run.fail_reason.trim().to_string()),
                run.note.trim(),
                now_ms()
            ],
        )?;
        self.list_print_runs(project_id)?.into_iter().find(|r| r.id == id).ok_or_else(|| DbError::NotFound(format!("print run {id}")))
    }

    pub fn delete_print_run(&self, id: &str) -> DbResult<()> {
        let n = self
            .w()
            .execute("UPDATE print_runs SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, now_ms()])?;
        if n == 0 {
            return Err(DbError::NotFound(format!("print run {id}")));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- 给定价页的上下文

    /// 调研给的竞品价格带(元):这个项目采用自哪张机会卡,就用那张的;否则取名下调研里所有机会的范围。
    pub fn project_price_band(&self, project_id: &str) -> DbResult<Option<(u32, u32)>> {
        let conn = self.r();
        let read = |sql: &str| -> DbResult<Option<(u32, u32)>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([project_id], |r| r.get::<_, String>(0))?;
            let mut band: Option<(u32, u32)> = None;
            for json in rows {
                let v: serde_json::Value = serde_json::from_str(&json?).unwrap_or_default();
                let (lo, hi) = (v["price_low"].as_u64().unwrap_or(0) as u32, v["price_high"].as_u64().unwrap_or(0) as u32);
                if lo > 0 && hi >= lo {
                    band = Some(band.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
                }
            }
            Ok(band)
        };
        if let Some(band) = read("SELECT detail_json FROM opportunities WHERE adopted_project_id = ?1")? {
            return Ok(Some(band));
        }
        read("SELECT o.detail_json FROM opportunities o JOIN research_runs r ON r.id = o.research_run_id WHERE r.project_id = ?1")
    }

    /// 项目名下已经建出来的模型的体积与表面积(几个零件就合计),用来估克数和时长。
    pub fn project_model_geometry(&self, project_id: &str) -> DbResult<Option<ModelGeometry>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT v.metrics_json FROM cad_designs d JOIN cad_versions v ON v.id = d.current_version_id
             WHERE d.project_id = ?1 AND d.deleted_at IS NULL AND v.deleted_at IS NULL",
        )?;
        let rows = stmt.query_map([project_id], |r| r.get::<_, String>(0))?;
        let mut sum = ModelGeometry::default();
        for json in rows {
            if let Ok(m) = serde_json::from_str::<pp_common::cad::CadMetrics>(&json?) {
                sum.volume_mm3 += m.volume_mm3;
                sum.area_mm2 += m.area_mm2;
                sum.designs += 1;
            }
        }
        Ok((sum.designs > 0).then_some(sum))
    }

    /// 项目中枢 / 定价页列表里的一行摘要。
    pub fn pricing_summary(&self, project_id: &str) -> DbResult<PricingSummary> {
        let conn = self.r();
        let model: Option<(i64, Option<i64>)> = conn
            .query_row("SELECT unit_cost, chosen_price FROM cost_models WHERE project_id = ?1", [project_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let (runs, successes): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(result = 'success'), 0) FROM print_runs WHERE project_id = ?1 AND deleted_at IS NULL",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(PricingSummary {
            unit_cost: model.map(|m| to_yuan(m.0)).unwrap_or(0.0),
            chosen_price: model.and_then(|m| m.1).map(to_yuan),
            runs: runs.max(0) as u32,
            successes: successes.max(0) as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use pp_common::cost::{breakdown, CostDefaults, Material, PrintRun, Printer};
    use pp_common::gate::gate_passed;
    use pp_common::{NewProject, Stage};

    use crate::testutil::TempDb;
    use crate::DbError;

    fn project(t: &TempDb) -> String {
        t.create_project(&NewProject {
            title: "线缆夹".into(),
            category: String::new(),
            hypothesis: "桌面党会为理线付 19 元".into(),
        })
        .unwrap()
        .id
    }

    fn run(success: bool, grams: f64, hours: f64, reason: &str) -> PrintRun {
        PrintRun {
            id: String::new(),
            project_id: String::new(),
            printer_id: None,
            material_id: None,
            est_hours: Some(1.7),
            est_grams: Some(27.0),
            actual_hours: Some(hours),
            actual_grams: Some(grams),
            success,
            fail_reason: reason.into(),
            note: String::new(),
            created_at: 0,
        }
    }

    #[test]
    fn profiles_roundtrip_in_yuan_and_are_soft_deleted() {
        let t = TempDb::new();
        let p = t
            .save_printer(&Printer {
                name: " A1 mini ".into(),
                price_yuan: 1899.5,
                power_w: 85.0,
                ..Default::default()
            })
            .unwrap();
        assert_eq!((p.name.as_str(), p.price_yuan, p.power_w), ("A1 mini", 1899.5, 85.0));
        let edited = t.save_printer(&Printer { name: "A1 mini(客厅)".into(), ..p.clone() }).unwrap();
        assert_eq!((edited.id.as_str(), t.list_printers().unwrap().len()), (p.id.as_str(), 1), "带 id 保存是更新,不是再建一台");
        assert!(t.save_printer(&Printer { name: "  ".into(), ..Default::default() }).is_err());
        assert!(t.save_printer(&Printer { name: "x".into(), lifetime_hours: 0.0, ..Default::default() }).is_err(), "寿命为 0 会让折旧除以 0");

        let m = t.save_material(&Material { name: "哑光 PLA 白".into(), yuan_per_kg: 79.9, ..Default::default() }).unwrap();
        assert_eq!((m.kind.as_str(), m.yuan_per_kg), ("PLA", 79.9));
        t.delete_material(&m.id).unwrap();
        assert!(t.list_materials().unwrap().is_empty());
        assert!(matches!(t.delete_material(&m.id), Err(DbError::NotFound(_))));
        t.delete_printer(&p.id).unwrap();
        assert!(matches!(t.save_printer(&edited), Err(DbError::NotFound(_))), "删掉的档案不能靠保存复活");
    }

    #[test]
    fn a_project_starts_from_the_library_defaults_and_keeps_its_own_numbers() {
        let t = TempDb::new();
        let p = project(&t);
        assert_eq!(t.cost_defaults().unwrap(), CostDefaults::default(), "没存过 = 出厂默认值");
        let mut d = CostDefaults::default();
        d.electricity_yuan_per_kwh = 0.52;
        d.labor_yuan_per_min = 0.8;
        t.set_cost_defaults(&d).unwrap();
        d.success_rate = 0.0;
        assert!(t.set_cost_defaults(&d).is_err());

        let draft = t.cost_model(&p).unwrap();
        assert!(!draft.saved && draft.chosen_price.is_none());
        assert_eq!((draft.params.electricity_yuan_per_kwh, draft.params.labor_yuan_per_min), (0.52, 0.8), "草稿用的是这个资料库自己的默认值");

        let mut params = draft.params.clone();
        params.grams = 27.0;
        params.print_hours = 1.7;
        let unit = breakdown(&params).unit_cost;
        let saved = t.save_cost_model(&p, &params, unit, Some(29.9)).unwrap();
        assert!(saved.saved && saved.params == params);
        assert_eq!(saved.chosen_price, Some(29.9));
        let s = t.pricing_summary(&p).unwrap();
        assert!((s.unit_cost - (unit * 100.0).round() / 100.0).abs() < 1e-9 && s.chosen_price == Some(29.9));

        // 改了资料库默认值,不影响已经存过的项目
        t.set_cost_defaults(&CostDefaults::default()).unwrap();
        assert_eq!(t.cost_model(&p).unwrap().params.labor_yuan_per_min, 0.8);
        // 取消定价
        assert_eq!(t.save_cost_model(&p, &params, unit, None).unwrap().chosen_price, None);
        assert!(matches!(t.cost_model("nope"), Err(DbError::NotFound(_))));
    }

    #[test]
    fn print_runs_are_logged_with_estimates_and_failures_need_a_reason() {
        let t = TempDb::new();
        let p = project(&t);
        assert!(matches!(t.add_print_run(&p, &run(false, 12.0, 0.6, "  ")), Err(DbError::Invalid(_))), "失败必须写原因");
        let failed = t.add_print_run(&p, &run(false, 12.0, 0.6, "翘边,第 3 层脱床")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let ok = t.add_print_run(&p, &run(true, 29.5, 1.9, "这一栏成功时不存")).unwrap();
        assert!(ok.fail_reason.is_empty());
        assert!((ok.actual_hours.unwrap() - 1.9).abs() < 1e-9 && ok.est_grams == Some(27.0), "小时 ↔ 分钟往返不丢精度");

        let runs = t.list_print_runs(&p).unwrap();
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [ok.id.as_str(), failed.id.as_str()], "最新的在前");
        let s = t.pricing_summary(&p).unwrap();
        assert_eq!((s.runs, s.successes), (2, 1));

        t.delete_print_run(&ok.id).unwrap();
        assert_eq!(t.pricing_summary(&p).unwrap().successes, 0);
        assert!(matches!(t.delete_print_run(&ok.id), Err(DbError::NotFound(_))));
    }

    #[test]
    fn a_successful_run_plus_a_price_closes_the_prototype_gate() {
        let t = TempDb::new();
        let p = project(&t);
        assert!(!gate_passed(Stage::Prototype, &t.project_facts(&p).unwrap()));
        t.add_print_run(&p, &run(false, 10.0, 0.5, "堵头")).unwrap();
        let params = t.cost_model(&p).unwrap().params;
        t.save_cost_model(&p, &params, 12.0, None).unwrap();
        let f = t.project_facts(&p).unwrap();
        assert_eq!((f.print_successes, f.has_price), (0, false), "失败的打样、没定价的模型都不算");

        t.add_print_run(&p, &run(true, 27.0, 1.7, "")).unwrap();
        t.save_cost_model(&p, &params, 12.0, Some(29.9)).unwrap();
        let f = t.project_facts(&p).unwrap();
        assert_eq!((f.print_successes, f.has_price), (1, true));
        assert!(gate_passed(Stage::Prototype, &f));
    }

    #[test]
    fn the_price_band_prefers_the_adopted_opportunity() {
        let t = TempDb::new();
        let p = project(&t);
        assert_eq!(t.project_price_band(&p).unwrap(), None);
        assert_eq!(t.project_model_geometry(&p).unwrap(), None);

        // 为这个项目做的调研:两张机会卡 ¥19~39 和 ¥19~89 → 取整个范围
        let (brief, mut report) = crate::research::tests::sample();
        report.opportunities[1].price_high = 89;
        let run_id = t.save_research(Some(&p), &brief, &report).unwrap();
        assert_eq!(t.project_price_band(&p).unwrap(), Some((19, 89)));

        // 从第一张机会卡采用出来的新项目:只认它自己那张卡的价格带
        let adopted = t.adopt_opportunity(&run_id, 0).unwrap();
        assert_eq!(t.project_price_band(&adopted.id).unwrap(), Some((19, 39)));
    }
}
