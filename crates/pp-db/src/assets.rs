use pp_common::{Asset, AssetKind};
use rusqlite::{params, OptionalExtension, Row};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const COLS: &str = "id, project_id, kind, role, rel_path, ext, bytes, meta_json, parent_asset_id, \
                    source_job_id, ai_generated, is_adopted, created_at";

#[derive(Debug, Clone)]
pub struct NewAsset {
    /// 由调用方先生成(文件名要用它),见 `pp_db::new_id`
    pub id: String,
    pub project_id: Option<String>,
    pub kind: AssetKind,
    pub role: String,
    pub rel_path: String,
    pub ext: String,
    pub bytes: i64,
    pub meta_json: Option<String>,
    pub parent_asset_id: Option<String>,
    pub source_job_id: Option<String>,
    pub ai_generated: bool,
}

impl NewAsset {
    pub fn new(kind: AssetKind, role: &str, rel_path: &str, ext: &str, bytes: i64) -> Self {
        Self {
            id: new_id(),
            project_id: None,
            kind,
            role: role.to_string(),
            rel_path: rel_path.to_string(),
            ext: ext.to_ascii_lowercase(),
            bytes,
            meta_json: None,
            parent_asset_id: None,
            source_job_id: None,
            ai_generated: false,
        }
    }
}

fn row_to_asset(r: &Row<'_>) -> rusqlite::Result<Asset> {
    let kind: String = r.get(2)?;
    Ok(Asset {
        id: r.get(0)?,
        project_id: r.get(1)?,
        kind: AssetKind::parse(&kind).unwrap_or(AssetKind::Doc),
        role: r.get(3)?,
        rel_path: r.get(4)?,
        ext: r.get(5)?,
        bytes: r.get(6)?,
        meta_json: r.get(7)?,
        parent_asset_id: r.get(8)?,
        source_job_id: r.get(9)?,
        ai_generated: r.get(10)?,
        is_adopted: r.get(11)?,
        created_at: r.get(12)?,
    })
}

/// 资产路径只允许落在资产库里面:相对路径、正斜杠、不含 `..`。
/// pp-asset:// 协议靠「URL 里只有 ID,路径查库得到」杜绝穿越,这里再把入库这一头也堵上。
fn check_rel_path(p: &str) -> DbResult<()> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.contains(':')
        || p.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..");
    if bad {
        return Err(DbError::Invalid(format!("bad asset path: {p}")));
    }
    Ok(())
}

impl Db {
    pub fn insert_asset(&self, a: &NewAsset) -> DbResult<Asset> {
        check_rel_path(&a.rel_path)?;
        self.w().execute(
            "INSERT INTO assets(id, project_id, kind, role, rel_path, ext, bytes, meta_json,
                                parent_asset_id, source_job_id, ai_generated, is_adopted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                a.id,
                a.project_id,
                a.kind.as_str(),
                a.role,
                a.rel_path,
                a.ext,
                a.bytes,
                a.meta_json,
                a.parent_asset_id,
                a.source_job_id,
                a.ai_generated,
                now_ms()
            ],
        )?;
        self.get_asset(&a.id)
    }

    pub fn get_asset(&self, id: &str) -> DbResult<Asset> {
        self.r()
            .query_row(
                &format!("SELECT {COLS} FROM assets WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                row_to_asset,
            )
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("asset {id}")))
    }

    /// `project_id = None` 列出不属于任何项目的资产(灵感池、预研样例)。
    pub fn list_assets(&self, project_id: Option<&str>, kind: Option<AssetKind>) -> DbResult<Vec<Asset>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM assets
             WHERE deleted_at IS NULL
               AND ((?1 IS NULL AND project_id IS NULL) OR project_id = ?1)
               AND (?2 IS NULL OR kind = ?2)
             ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params![project_id, kind.map(AssetKind::as_str)], row_to_asset)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_asset_meta(&self, id: &str, meta_json: &str) -> DbResult<()> {
        let n = self.w().execute(
            "UPDATE assets SET meta_json = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, meta_json],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("asset {id}")));
        }
        Ok(())
    }

    pub fn delete_asset(&self, id: &str) -> DbResult<()> {
        let n = self.w().execute(
            "UPDATE assets SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("asset {id}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;

    #[test]
    fn insert_then_read_back() {
        let t = TempDb::new();
        let mut a = NewAsset::new(AssetKind::Model3d, "model_raw", "assets/lab/x.STL", "STL", 84);
        a.ai_generated = true;
        let saved = t.insert_asset(&a).unwrap();
        assert_eq!(saved.id, a.id);
        assert_eq!(saved.ext, "stl", "扩展名统一成小写");
        assert!(saved.ai_generated);
        assert!(!saved.is_adopted);
        assert_eq!(t.get_asset(&a.id).unwrap(), saved);
    }

    #[test]
    fn paths_that_escape_the_library_are_rejected() {
        let t = TempDb::new();
        for bad in [
            "",
            "/etc/passwd",
            "C:/Windows/x.stl",
            "assets\\x.stl",
            "assets/../../x.stl",
            "assets//x.stl",
            "./x.stl",
        ] {
            let a = NewAsset::new(AssetKind::Model3d, "model_raw", bad, "stl", 1);
            assert!(
                matches!(t.insert_asset(&a), Err(DbError::Invalid(_))),
                "应当拒绝 {bad:?}"
            );
        }
    }

    #[test]
    fn listing_filters_by_project_and_kind() {
        let t = TempDb::new();
        let p = t
            .create_project(&pp_common::NewProject {
                title: "p".into(),
                ..Default::default()
            })
            .unwrap();
        let mut in_project = NewAsset::new(AssetKind::Image, "concept_ref", "assets/PP-0001/a.png", "png", 1);
        in_project.project_id = Some(p.id.clone());
        t.insert_asset(&in_project).unwrap();
        let loose = NewAsset::new(AssetKind::Model3d, "model_raw", "assets/lab/b.stl", "stl", 1);
        t.insert_asset(&loose).unwrap();

        let of_project = t.list_assets(Some(&p.id), None).unwrap();
        assert_eq!(of_project.len(), 1);
        assert_eq!(of_project[0].id, in_project.id);

        let loose_models = t.list_assets(None, Some(AssetKind::Model3d)).unwrap();
        assert_eq!(loose_models.len(), 1);
        assert_eq!(loose_models[0].id, loose.id);

        assert!(t.list_assets(None, Some(AssetKind::Image)).unwrap().is_empty());
    }

    #[test]
    fn deleted_assets_are_gone() {
        let t = TempDb::new();
        let a = NewAsset::new(AssetKind::Image, "cover", "assets/lab/c.png", "png", 1);
        t.insert_asset(&a).unwrap();
        t.delete_asset(&a.id).unwrap();
        assert!(matches!(t.get_asset(&a.id), Err(DbError::NotFound(_))));
        assert!(matches!(t.delete_asset(&a.id), Err(DbError::NotFound(_))));
    }
}
