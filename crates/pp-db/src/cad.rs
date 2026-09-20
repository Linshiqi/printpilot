//! 代码式 CAD 的版本库:每次生成、改参数、指令修补都存成一个新版本,`parent_id` 串成版本树。

use pp_common::cad::{CadBuildReport, CadMetrics, CadParam, CadVersion, DesignSpec};
use rusqlite::{params, OptionalExtension, Row};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const COLS: &str = "id, project_id, parent_id, source, note, code, params_json, metrics_json, \
                    stl_asset_id, step_asset_id, elapsed_ms, created_at, spec_json, ref_asset_ids_json, report_json";

#[derive(Debug, Clone)]
pub struct NewCadVersion {
    pub project_id: Option<String>,
    pub parent_id: Option<String>,
    pub source: String,
    pub note: String,
    pub code: String,
    pub spec: Option<DesignSpec>,
    pub ref_asset_ids: Vec<String>,
    pub report: Option<CadBuildReport>,
    pub params: Vec<CadParam>,
    pub metrics: CadMetrics,
    pub stl_asset_id: String,
    pub step_asset_id: Option<String>,
    pub elapsed_ms: u64,
}

fn row_to_version(r: &Row<'_>) -> rusqlite::Result<CadVersion> {
    let params_json: String = r.get(6)?;
    let metrics_json: String = r.get(7)?;
    let spec_json: Option<String> = r.get(12)?;
    let refs_json: String = r.get(13)?;
    let report_json: Option<String> = r.get(14)?;
    Ok(CadVersion {
        id: r.get(0)?,
        project_id: r.get(1)?,
        parent_id: r.get(2)?,
        source: r.get(3)?,
        note: r.get(4)?,
        code: r.get(5)?,
        // 这几列是我们自己写进去的 JSON;万一读不出来,宁可给空值也别让整个列表打不开
        spec: spec_json.and_then(|j| serde_json::from_str(&j).ok()),
        ref_asset_ids: serde_json::from_str(&refs_json).unwrap_or_default(),
        report: report_json.and_then(|j| serde_json::from_str(&j).ok()),
        params: serde_json::from_str(&params_json).unwrap_or_default(),
        metrics: serde_json::from_str(&metrics_json).unwrap_or_default(),
        stl_asset_id: r.get(8)?,
        step_asset_id: r.get(9)?,
        elapsed_ms: r.get::<_, i64>(10)?.max(0) as u64,
        created_at: r.get(11)?,
    })
}

impl Db {
    pub fn insert_cad_version(&self, v: &NewCadVersion) -> DbResult<CadVersion> {
        let id = new_id();
        let json = |e: serde_json::Error| DbError::Invalid(format!("cad json: {e}"));
        self.w().execute(
            &format!(
                "INSERT INTO cad_versions({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
            ),
            params![
                id,
                v.project_id,
                v.parent_id,
                v.source,
                v.note,
                v.code,
                serde_json::to_string(&v.params).map_err(json)?,
                serde_json::to_string(&v.metrics).map_err(json)?,
                v.stl_asset_id,
                v.step_asset_id,
                v.elapsed_ms as i64,
                now_ms(),
                v.spec.as_ref().map(serde_json::to_string).transpose().map_err(json)?,
                serde_json::to_string(&v.ref_asset_ids).map_err(json)?,
                v.report.as_ref().map(serde_json::to_string).transpose().map_err(json)?
            ],
        )?;
        self.get_cad_version(&id)
    }

    pub fn get_cad_version(&self, id: &str) -> DbResult<CadVersion> {
        self.r()
            .query_row(
                &format!("SELECT {COLS} FROM cad_versions WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                row_to_version,
            )
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("cad version {id}")))
    }

    /// 最新的在前。`project_id = None` 列出不属于任何项目的版本(预研页里做的)。
    pub fn list_cad_versions(&self, project_id: Option<&str>) -> DbResult<Vec<CadVersion>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM cad_versions
             WHERE deleted_at IS NULL AND ((?1 IS NULL AND project_id IS NULL) OR project_id = ?1)
             ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params![project_id], row_to_version)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn delete_cad_version(&self, id: &str) -> DbResult<()> {
        let n = self.w().execute(
            "UPDATE cad_versions SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("cad version {id}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;
    use crate::NewAsset;
    use pp_common::AssetKind;

    fn asset(t: &TempDb, name: &str) -> String {
        let a = NewAsset::new(AssetKind::Model3d, "cad_mesh", &format!("assets/lab/{name}.stl"), "stl", 1);
        t.insert_asset(&a).unwrap().id
    }

    fn version(t: &TempDb, parent: Option<&str>, source: &str) -> CadVersion {
        let stl = asset(t, &new_id());
        t.insert_cad_version(&NewCadVersion {
            project_id: None,
            parent_id: parent.map(str::to_string),
            source: source.into(),
            note: "宽度 60 → 90".into(),
            code: "result = Box(90, 40, 6)\n".into(),
            spec: Some(DesignSpec {
                name: "支架".into(),
                overall_mm: [90.0, 40.0, 6.0],
                ..Default::default()
            }),
            ref_asset_ids: vec!["ref-1".into()],
            report: Some(CadBuildReport {
                llm_calls: 2,
                changed_sections: vec!["PARAMS".into()],
                warnings: vec![pp_common::cad::CadProblem::OffPlate { z: -3.0 }],
                ..Default::default()
            }),
            params: vec![CadParam {
                name: "width".into(),
                value: 90.0,
                unit: "mm".into(),
                label: "总宽".into(),
                min: Some(20.0),
                max: Some(200.0),
                line: 4,
                integer: false,
            }],
            metrics: CadMetrics {
                size: [90.0, 40.0, 6.0],
                solids: 1,
                is_valid: true,
                ..Default::default()
            },
            stl_asset_id: stl,
            step_asset_id: None,
            elapsed_ms: 812,
        })
        .unwrap()
    }

    #[test]
    fn versions_roundtrip_with_params_and_metrics() {
        let t = TempDb::new();
        let v = version(&t, None, "manual");
        let back = t.get_cad_version(&v.id).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.params[0].label, "总宽");
        assert_eq!(back.metrics.size, [90.0, 40.0, 6.0]);
        assert_eq!(back.elapsed_ms, 812);
        // 规格、参考图、过程报告跟着版本走:复核与「这一版改了哪里」都靠它们
        assert_eq!(back.spec.as_ref().unwrap().name, "支架");
        assert_eq!(back.ref_asset_ids, ["ref-1"]);
        let report = back.report.unwrap();
        assert_eq!(report.changed_sections, ["PARAMS"]);
        assert_eq!(report.warnings, [pp_common::cad::CadProblem::OffPlate { z: -3.0 }]);
    }

    #[test]
    fn a_param_edit_links_back_to_the_version_it_came_from() {
        let t = TempDb::new();
        let root = version(&t, None, "generate");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let child = version(&t, Some(&root.id), "param");
        assert_eq!(child.parent_id.as_deref(), Some(root.id.as_str()));
        let list = t.list_cad_versions(None).unwrap();
        assert_eq!(list.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), [child.id.as_str(), root.id.as_str()]);
    }

    #[test]
    fn a_version_must_point_at_a_real_mesh_asset() {
        let t = TempDb::new();
        let mut bad = NewCadVersion {
            project_id: None,
            parent_id: None,
            source: "manual".into(),
            note: String::new(),
            code: "x".into(),
            spec: None,
            ref_asset_ids: vec![],
            report: None,
            params: vec![],
            metrics: CadMetrics::default(),
            stl_asset_id: "no-such-asset".into(),
            step_asset_id: None,
            elapsed_ms: 0,
        };
        assert!(t.insert_cad_version(&bad).is_err(), "外键应当拦住悬空的资产引用");
        bad.stl_asset_id = asset(&t, "ok");
        assert!(t.insert_cad_version(&bad).is_ok());
    }

    #[test]
    fn deleted_versions_disappear() {
        let t = TempDb::new();
        let v = version(&t, None, "manual");
        t.delete_cad_version(&v.id).unwrap();
        assert!(matches!(t.get_cad_version(&v.id), Err(DbError::NotFound(_))));
        assert!(t.list_cad_versions(None).unwrap().is_empty());
    }
}
