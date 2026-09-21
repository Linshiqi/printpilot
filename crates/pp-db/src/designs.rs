//! 建模工作室的仓储:设计、对话与时间线。

use pp_common::cad::DesignSpec;
use pp_common::design::{CadDesign, CadDesignSummary, CadMessage, MsgExtra, MsgKind, MsgRole};
use rusqlite::{params, OptionalExtension, Row};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const DESIGN_COLS: &str = "id, project_id, name, spec_json, ref_asset_ids_json, current_version_id, thumb, created_at, updated_at";
const MSG_COLS: &str = "id, design_id, role, kind, content, extra_json, image_asset_ids_json, version_id, created_at";

/// 缩略图是 data URL,存在库里:限制大小,别让一张图把列表查询拖慢。
const MAX_THUMB_BYTES: usize = 60_000;

#[derive(Debug, Clone)]
pub struct NewCadMessage {
    pub design_id: String,
    pub role: MsgRole,
    pub kind: MsgKind,
    pub content: String,
    pub extra: MsgExtra,
    pub image_asset_ids: Vec<String>,
    pub version_id: Option<String>,
}

impl NewCadMessage {
    pub fn new(design_id: &str, role: MsgRole, kind: MsgKind, content: impl Into<String>) -> Self {
        Self {
            design_id: design_id.to_string(),
            role,
            kind,
            content: content.into(),
            extra: MsgExtra::default(),
            image_asset_ids: Vec::new(),
            version_id: None,
        }
    }
}

fn row_to_design(r: &Row<'_>) -> rusqlite::Result<CadDesign> {
    let spec_json: Option<String> = r.get(3)?;
    let refs_json: String = r.get(4)?;
    Ok(CadDesign {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        spec: spec_json.and_then(|j| serde_json::from_str(&j).ok()),
        ref_asset_ids: serde_json::from_str(&refs_json).unwrap_or_default(),
        current_version_id: r.get(5)?,
        thumb: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn row_to_message(r: &Row<'_>) -> rusqlite::Result<CadMessage> {
    let role: String = r.get(2)?;
    let kind: String = r.get(3)?;
    let extra_json: String = r.get(5)?;
    let images_json: String = r.get(6)?;
    Ok(CadMessage {
        id: r.get(0)?,
        design_id: r.get(1)?,
        role: MsgRole::parse(&role),
        kind: MsgKind::parse(&kind),
        content: r.get(4)?,
        extra: serde_json::from_str(&extra_json).unwrap_or_default(),
        image_asset_ids: serde_json::from_str(&images_json).unwrap_or_default(),
        version_id: r.get(7)?,
        created_at: r.get(8)?,
    })
}

fn json_err(e: serde_json::Error) -> DbError {
    DbError::Invalid(format!("design json: {e}"))
}

impl Db {
    pub fn create_design(&self, name: &str, project_id: Option<&str>) -> DbResult<CadDesign> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Invalid("design name is empty".into()));
        }
        let (id, at) = (new_id(), now_ms());
        self.w().execute(
            "INSERT INTO cad_designs(id, project_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, project_id, name, at],
        )?;
        self.get_design(&id)
    }

    pub fn get_design(&self, id: &str) -> DbResult<CadDesign> {
        self.r()
            .query_row(
                &format!("SELECT {DESIGN_COLS} FROM cad_designs WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                row_to_design,
            )
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("design {id}")))
    }

    /// 最近动过的在前。带上当前版本的尺寸与版本数,列表不用再逐个去查。
    pub fn list_designs(&self) -> DbResult<Vec<CadDesignSummary>> {
        self.designs_where(None)
    }

    /// 某个项目名下的设计。
    pub fn list_designs_of_project(&self, project_id: &str) -> DbResult<Vec<CadDesignSummary>> {
        self.designs_where(Some(project_id))
    }

    fn designs_where(&self, project_id: Option<&str>) -> DbResult<Vec<CadDesignSummary>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.name, d.project_id, d.thumb, d.updated_at,
                    (SELECT metrics_json FROM cad_versions v WHERE v.id = d.current_version_id AND v.deleted_at IS NULL),
                    (SELECT COUNT(*) FROM cad_versions v WHERE v.design_id = d.id AND v.deleted_at IS NULL)
             FROM cad_designs d WHERE d.deleted_at IS NULL AND (?1 IS NULL OR d.project_id = ?1)
             ORDER BY d.updated_at DESC, d.id DESC",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            let metrics: Option<String> = r.get(5)?;
            let size = metrics
                .and_then(|j| serde_json::from_str::<pp_common::cad::CadMetrics>(&j).ok())
                .map(|m| m.size);
            Ok(CadDesignSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                project_id: r.get(2)?,
                thumb: r.get(3)?,
                size,
                versions: r.get::<_, i64>(6)?.max(0) as u32,
                updated_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn touch_design(&self, id: &str, sql_set: &str, value: &dyn rusqlite::ToSql) -> DbResult<CadDesign> {
        let n = self.w().execute(
            &format!("UPDATE cad_designs SET {sql_set} = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL"),
            params![id, value, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("design {id}")));
        }
        self.get_design(id)
    }

    pub fn rename_design(&self, id: &str, name: &str) -> DbResult<CadDesign> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Invalid("design name is empty".into()));
        }
        self.touch_design(id, "name", &name)
    }

    pub fn link_design_project(&self, id: &str, project_id: Option<&str>) -> DbResult<CadDesign> {
        self.touch_design(id, "project_id", &project_id)
    }

    pub fn set_design_spec(&self, id: &str, spec: Option<&DesignSpec>) -> DbResult<CadDesign> {
        let json = spec.map(serde_json::to_string).transpose().map_err(json_err)?;
        self.touch_design(id, "spec_json", &json)
    }

    pub fn set_design_refs(&self, id: &str, ref_asset_ids: &[String]) -> DbResult<CadDesign> {
        let json = serde_json::to_string(ref_asset_ids).map_err(json_err)?;
        self.touch_design(id, "ref_asset_ids_json", &json)
    }

    /// 选中某个版本(必须是这个设计自己的版本)。
    pub fn set_design_current(&self, id: &str, version_id: Option<&str>) -> DbResult<CadDesign> {
        if let Some(v) = version_id {
            let owned: i64 = self.r().query_row(
                "SELECT COUNT(*) FROM cad_versions WHERE id = ?1 AND design_id = ?2 AND deleted_at IS NULL",
                params![v, id],
                |r| r.get(0),
            )?;
            if owned == 0 {
                return Err(DbError::Invalid(format!("version {v} does not belong to design {id}")));
            }
        }
        self.touch_design(id, "current_version_id", &version_id)
    }

    /// 缩略图不算「动过这个设计」:不更新 updated_at,否则点开看一眼就会让它跳到列表最前面。
    pub fn set_design_thumb(&self, id: &str, thumb: &str) -> DbResult<()> {
        if !thumb.starts_with("data:image/") || thumb.len() > MAX_THUMB_BYTES {
            return Err(DbError::Invalid("thumbnail must be a small image data URL".into()));
        }
        self.w()
            .execute("UPDATE cad_designs SET thumb = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, thumb])?;
        Ok(())
    }

    pub fn delete_design(&self, id: &str) -> DbResult<()> {
        let n = self.w().execute(
            "UPDATE cad_designs SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("design {id}")));
        }
        Ok(())
    }

    pub fn insert_cad_message(&self, m: &NewCadMessage) -> DbResult<CadMessage> {
        let (id, at) = (new_id(), now_ms());
        let w = self.w();
        w.execute(
            &format!("INSERT INTO cad_messages({MSG_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"),
            params![
                id,
                m.design_id,
                m.role.as_str(),
                m.kind.as_str(),
                m.content,
                serde_json::to_string(&m.extra).map_err(json_err)?,
                serde_json::to_string(&m.image_asset_ids).map_err(json_err)?,
                m.version_id,
                at
            ],
        )?;
        w.execute("UPDATE cad_designs SET updated_at = ?2 WHERE id = ?1", params![m.design_id, at])?;
        drop(w);
        Ok(CadMessage {
            id,
            design_id: m.design_id.clone(),
            role: m.role,
            kind: m.kind,
            content: m.content.clone(),
            extra: m.extra.clone(),
            image_asset_ids: m.image_asset_ids.clone(),
            version_id: m.version_id.clone(),
            created_at: at,
        })
    }

    /// 时间线,从早到晚。
    pub fn list_cad_messages(&self, design_id: &str) -> DbResult<Vec<CadMessage>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MSG_COLS} FROM cad_messages WHERE design_id = ?1 ORDER BY created_at ASC, id ASC"
        ))?;
        let rows = stmt.query_map([design_id], row_to_message)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;
    use crate::{NewAsset, NewCadVersion};
    use pp_common::cad::{CadMetrics, CadPick};
    use pp_common::AssetKind;

    fn version(t: &TempDb, design_id: &str, size: [f64; 3]) -> String {
        let a = NewAsset::new(AssetKind::Model3d, "cad_mesh", &format!("assets/lab/{}.stl", new_id()), "stl", 1);
        let stl = t.insert_asset(&a).unwrap().id;
        t.insert_cad_version(&NewCadVersion {
            design_id: Some(design_id.to_string()),
            project_id: None,
            parent_id: None,
            source: "generate".into(),
            note: String::new(),
            code: "result = 1".into(),
            spec: None,
            ref_asset_ids: vec![],
            report: None,
            params: vec![],
            metrics: CadMetrics {
                size,
                solids: 1,
                is_valid: true,
                ..Default::default()
            },
            stl_asset_id: stl,
            step_asset_id: None,
            elapsed_ms: 1,
        })
        .unwrap()
        .id
    }

    #[test]
    fn a_design_collects_spec_references_and_a_current_version() {
        let t = TempDb::new();
        let d = t.create_design("  线缆夹 ", None).unwrap();
        assert_eq!(d.name, "线缆夹");
        assert!(d.spec.is_none() && d.current_version_id.is_none());
        assert!(t.create_design("   ", None).is_err());

        let spec = DesignSpec {
            name: "线缆夹".into(),
            overall_mm: [60.0, 24.0, 16.0],
            ..Default::default()
        };
        t.set_design_spec(&d.id, Some(&spec)).unwrap();
        t.set_design_refs(&d.id, &["ref-1".to_string()]).unwrap();
        let v = version(&t, &d.id, [60.0, 24.0, 16.0]);
        let d = t.set_design_current(&d.id, Some(&v)).unwrap();
        assert_eq!(d.spec.unwrap().overall_mm, [60.0, 24.0, 16.0]);
        assert_eq!(d.ref_asset_ids, ["ref-1"]);
        assert_eq!(d.current_version_id.as_deref(), Some(v.as_str()));
        assert_eq!(t.list_design_versions(&d.id).unwrap().len(), 1);
    }

    #[test]
    fn a_design_can_only_point_at_its_own_versions() {
        let t = TempDb::new();
        let (a, b) = (t.create_design("A", None).unwrap(), t.create_design("B", None).unwrap());
        let v_of_b = version(&t, &b.id, [1.0, 1.0, 1.0]);
        assert!(matches!(t.set_design_current(&a.id, Some(&v_of_b)), Err(DbError::Invalid(_))));
        assert!(t.set_design_current(&a.id, None).is_ok());
    }

    #[test]
    fn the_list_is_ordered_by_activity_and_carries_size_and_counts() {
        let t = TempDb::new();
        let old = t.create_design("旧的", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let new = t.create_design("新的", None).unwrap();
        let v = version(&t, &old.id, [90.0, 24.0, 16.0]);
        version(&t, &old.id, [60.0, 24.0, 16.0]);
        t.set_design_current(&old.id, Some(&v)).unwrap();

        let list = t.list_designs().unwrap();
        assert_eq!(list[0].id, old.id, "刚动过的排最前");
        assert_eq!(list[0].size, Some([90.0, 24.0, 16.0]), "尺寸取的是当前选中的版本");
        assert_eq!(list[0].versions, 2);
        assert_eq!((list[1].id.as_str(), list[1].size, list[1].versions), (new.id.as_str(), None, 0));

        t.delete_design(&old.id).unwrap();
        assert_eq!(t.list_designs().unwrap().len(), 1);
        assert!(matches!(t.get_design(&old.id), Err(DbError::NotFound(_))));
    }

    #[test]
    fn the_timeline_keeps_order_extras_and_bumps_the_design() {
        let t = TempDb::new();
        let d = t.create_design("夹子", None).unwrap();
        let mut ask = NewCadMessage::new(&d.id, MsgRole::User, MsgKind::Text, "这里加个孔");
        ask.extra.pick = Some(CadPick {
            point: [1.0, 2.0, 3.0],
            normal: Some([0.0, 0.0, 1.0]),
        });
        ask.image_asset_ids = vec!["img-1".into()];
        let first = t.insert_cad_message(&ask).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let v = version(&t, &d.id, [60.0, 24.0, 16.0]);
        let mut done = NewCadMessage::new(&d.id, MsgRole::Assistant, MsgKind::Build, "已在顶面加了一个 4 mm 的孔");
        done.version_id = Some(v.clone());
        t.insert_cad_message(&done).unwrap();

        let timeline = t.list_cad_messages(&d.id).unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0], first);
        assert_eq!(timeline[0].extra.pick.unwrap().point, [1.0, 2.0, 3.0]);
        assert_eq!(timeline[0].image_asset_ids, ["img-1"]);
        assert_eq!((timeline[1].role, timeline[1].kind), (MsgRole::Assistant, MsgKind::Build));
        assert_eq!(timeline[1].version_id.as_deref(), Some(v.as_str()));
        assert!(t.get_design(&d.id).unwrap().updated_at >= timeline[1].created_at, "说过话 = 动过这个设计");
    }

    #[test]
    fn thumbnails_must_be_small_image_data_urls_and_do_not_reorder_the_list() {
        let t = TempDb::new();
        let d = t.create_design("夹子", None).unwrap();
        let before = t.get_design(&d.id).unwrap().updated_at;
        std::thread::sleep(std::time::Duration::from_millis(3));
        t.set_design_thumb(&d.id, "data:image/jpeg;base64,AAAA").unwrap();
        let after = t.get_design(&d.id).unwrap();
        assert_eq!(after.thumb.as_deref(), Some("data:image/jpeg;base64,AAAA"));
        assert_eq!(after.updated_at, before);
        assert!(t.set_design_thumb(&d.id, "https://example.com/x.png").is_err());
        assert!(t.set_design_thumb(&d.id, &format!("data:image/png;base64,{}", "A".repeat(MAX_THUMB_BYTES))).is_err());
    }
}
