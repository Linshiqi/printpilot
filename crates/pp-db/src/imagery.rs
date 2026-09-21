//! 图片工作台的仓储:画板、生成过的每一张图、对话时间线。

use pp_common::imagery::{ImageAspect, ImageBoard, ImageBoardSummary, ImageMessage, ImageMsgExtra, ImageMsgKind, ImagePurpose, ImageVersion};
use rusqlite::{params, OptionalExtension, Row};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const BOARD_COLS: &str = "id, project_id, name, purpose, aspect, ref_asset_ids_json, current_image_id, created_at, updated_at";
const VERSION_COLS: &str = "id, board_id, parent_id, asset_id, prompt, mode, provider, model, aspect, adopted, created_at";
const MSG_COLS: &str = "id, board_id, from_user, kind, content, extra_json, image_asset_ids_json, created_at";

#[derive(Debug, Clone)]
pub struct NewImageVersion {
    pub board_id: String,
    pub parent_id: Option<String>,
    pub asset_id: String,
    pub prompt: String,
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub aspect: ImageAspect,
}

#[derive(Debug, Clone)]
pub struct NewImageMessage {
    pub board_id: String,
    pub from_user: bool,
    pub kind: ImageMsgKind,
    pub content: String,
    pub extra: ImageMsgExtra,
    pub image_asset_ids: Vec<String>,
}

fn json_err(e: serde_json::Error) -> DbError {
    DbError::Invalid(format!("imagery json: {e}"))
}

fn row_to_board(r: &Row<'_>) -> rusqlite::Result<ImageBoard> {
    let (purpose, aspect, refs): (String, String, String) = (r.get(3)?, r.get(4)?, r.get(5)?);
    Ok(ImageBoard {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        purpose: ImagePurpose::parse(&purpose),
        aspect: ImageAspect::parse(&aspect),
        ref_asset_ids: serde_json::from_str(&refs).unwrap_or_default(),
        current_image_id: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn row_to_version(r: &Row<'_>) -> rusqlite::Result<ImageVersion> {
    let aspect: String = r.get(8)?;
    Ok(ImageVersion {
        id: r.get(0)?,
        board_id: r.get(1)?,
        parent_id: r.get(2)?,
        asset_id: r.get(3)?,
        prompt: r.get(4)?,
        mode: r.get(5)?,
        provider: r.get(6)?,
        model: r.get(7)?,
        aspect: ImageAspect::parse(&aspect),
        adopted: r.get(9)?,
        created_at: r.get(10)?,
    })
}

fn row_to_message(r: &Row<'_>) -> rusqlite::Result<ImageMessage> {
    let (kind, extra, images): (String, String, String) = (r.get(3)?, r.get(5)?, r.get(6)?);
    Ok(ImageMessage {
        id: r.get(0)?,
        board_id: r.get(1)?,
        from_user: r.get(2)?,
        kind: if kind == "images" { ImageMsgKind::Images } else { ImageMsgKind::Text },
        content: r.get(4)?,
        extra: serde_json::from_str(&extra).unwrap_or_default(),
        image_asset_ids: serde_json::from_str(&images).unwrap_or_default(),
        created_at: r.get(7)?,
    })
}

impl Db {
    pub fn create_board(&self, name: &str, purpose: ImagePurpose, project_id: Option<&str>) -> DbResult<ImageBoard> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Invalid("board name is empty".into()));
        }
        let (id, at) = (new_id(), now_ms());
        self.w().execute(
            "INSERT INTO image_boards(id, project_id, name, purpose, aspect, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, project_id, name, purpose.as_str(), purpose.default_aspect().as_str(), at],
        )?;
        self.get_board(&id)
    }

    pub fn get_board(&self, id: &str) -> DbResult<ImageBoard> {
        self.r()
            .query_row(&format!("SELECT {BOARD_COLS} FROM image_boards WHERE id = ?1 AND deleted_at IS NULL"), [id], row_to_board)
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("image board {id}")))
    }

    /// 最近动过的在前;封面取当前选中的那张图。
    pub fn list_boards(&self) -> DbResult<Vec<ImageBoardSummary>> {
        let conn = self.r();
        let mut stmt = conn.prepare(
            "SELECT b.id, b.name, b.purpose, b.updated_at,
                    (SELECT v.asset_id FROM image_versions v WHERE v.id = b.current_image_id AND v.deleted_at IS NULL),
                    (SELECT COUNT(*) FROM image_versions v WHERE v.board_id = b.id AND v.deleted_at IS NULL)
             FROM image_boards b WHERE b.deleted_at IS NULL ORDER BY b.updated_at DESC, b.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let purpose: String = r.get(2)?;
            Ok(ImageBoardSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                purpose: ImagePurpose::parse(&purpose),
                cover_asset_id: r.get(4)?,
                images: r.get::<_, i64>(5)?.max(0) as u32,
                updated_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn touch_board(&self, id: &str, column: &str, value: &dyn rusqlite::ToSql) -> DbResult<ImageBoard> {
        let n = self.w().execute(
            &format!("UPDATE image_boards SET {column} = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL"),
            params![id, value, now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("image board {id}")));
        }
        self.get_board(id)
    }

    pub fn rename_board(&self, id: &str, name: &str) -> DbResult<ImageBoard> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Invalid("board name is empty".into()));
        }
        self.touch_board(id, "name", &name)
    }

    pub fn set_board_options(&self, id: &str, purpose: ImagePurpose, aspect: ImageAspect) -> DbResult<ImageBoard> {
        self.touch_board(id, "purpose", &purpose.as_str())?;
        self.touch_board(id, "aspect", &aspect.as_str())
    }

    pub fn link_board_project(&self, id: &str, project_id: Option<&str>) -> DbResult<ImageBoard> {
        self.touch_board(id, "project_id", &project_id)
    }

    pub fn set_board_refs(&self, id: &str, ref_asset_ids: &[String]) -> DbResult<ImageBoard> {
        let json = serde_json::to_string(ref_asset_ids).map_err(json_err)?;
        self.touch_board(id, "ref_asset_ids_json", &json)
    }

    /// 选中一张图(必须是这个画板自己的)。
    pub fn set_board_current(&self, id: &str, version_id: Option<&str>) -> DbResult<ImageBoard> {
        if let Some(v) = version_id {
            let owned: i64 = self
                .r()
                .query_row("SELECT COUNT(*) FROM image_versions WHERE id = ?1 AND board_id = ?2 AND deleted_at IS NULL", params![v, id], |r| r.get(0))?;
            if owned == 0 {
                return Err(DbError::Invalid(format!("image {v} does not belong to board {id}")));
            }
        }
        self.touch_board(id, "current_image_id", &version_id)
    }

    pub fn delete_board(&self, id: &str) -> DbResult<()> {
        let n = self
            .w()
            .execute("UPDATE image_boards SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, now_ms()])?;
        if n == 0 {
            return Err(DbError::NotFound(format!("image board {id}")));
        }
        Ok(())
    }

    pub fn insert_image_version(&self, v: &NewImageVersion) -> DbResult<ImageVersion> {
        let id = new_id();
        self.w().execute(
            &format!("INSERT INTO image_versions({VERSION_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10)"),
            params![id, v.board_id, v.parent_id, v.asset_id, v.prompt, v.mode, v.provider, v.model, v.aspect.as_str(), now_ms()],
        )?;
        self.get_image_version(&id)
    }

    pub fn get_image_version(&self, id: &str) -> DbResult<ImageVersion> {
        self.r()
            .query_row(
                &format!("SELECT {VERSION_COLS} FROM image_versions WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                row_to_version,
            )
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("image {id}")))
    }

    /// 最新的在前。
    pub fn list_image_versions(&self, board_id: &str) -> DbResult<Vec<ImageVersion>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {VERSION_COLS} FROM image_versions WHERE board_id = ?1 AND deleted_at IS NULL ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([board_id], row_to_version)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_image_adopted(&self, id: &str, adopted: bool) -> DbResult<ImageVersion> {
        self.w()
            .execute("UPDATE image_versions SET adopted = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, adopted])?;
        self.get_image_version(id)
    }

    /// 从画板上拿掉一张图(软删除;图片文件仍在资料库里)。拿掉的如果是当前选中的那张,改选剩下的里面最新的一张。
    /// 从它改出来的图不受影响——它们各自是完整的图,只是「父图」不再显示。
    pub fn remove_image_version(&self, id: &str) -> DbResult<ImageBoard> {
        let version = self.get_image_version(id)?;
        self.w().execute("UPDATE image_versions SET deleted_at = ?2 WHERE id = ?1", params![id, now_ms()])?;
        let board = self.get_board(&version.board_id)?;
        if board.current_image_id.as_deref() != Some(id) {
            return self.touch_board(&board.id, "current_image_id", &board.current_image_id);
        }
        let next = self.list_image_versions(&board.id)?.into_iter().next().map(|v| v.id);
        self.touch_board(&board.id, "current_image_id", &next)
    }

    pub fn insert_image_message(&self, m: &NewImageMessage) -> DbResult<ImageMessage> {
        let (id, at) = (new_id(), now_ms());
        let kind = if m.kind == ImageMsgKind::Images { "images" } else { "text" };
        let w = self.w();
        w.execute(
            &format!("INSERT INTO image_messages({MSG_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
            params![
                id,
                m.board_id,
                m.from_user,
                kind,
                m.content,
                serde_json::to_string(&m.extra).map_err(json_err)?,
                serde_json::to_string(&m.image_asset_ids).map_err(json_err)?,
                at
            ],
        )?;
        w.execute("UPDATE image_boards SET updated_at = ?2 WHERE id = ?1", params![m.board_id, at])?;
        drop(w);
        Ok(ImageMessage {
            id,
            board_id: m.board_id.clone(),
            from_user: m.from_user,
            kind: m.kind,
            content: m.content.clone(),
            extra: m.extra.clone(),
            image_asset_ids: m.image_asset_ids.clone(),
            created_at: at,
        })
    }

    /// 时间线,从早到晚。
    pub fn list_image_messages(&self, board_id: &str) -> DbResult<Vec<ImageMessage>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MSG_COLS} FROM image_messages WHERE board_id = ?1 ORDER BY created_at ASC, id ASC"
        ))?;
        let rows = stmt.query_map([board_id], row_to_message)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;
    use crate::NewAsset;
    use pp_common::AssetKind;

    fn image(t: &TempDb, board: &str, parent: Option<&str>, mode: &str) -> ImageVersion {
        let a = NewAsset::new(AssetKind::Image, "studio_image", &format!("assets/lab/{}.jpg", new_id()), "jpg", 1);
        let asset_id = t.insert_asset(&a).unwrap().id;
        t.insert_image_version(&NewImageVersion {
            board_id: board.to_string(),
            parent_id: parent.map(str::to_string),
            asset_id,
            prompt: "白底,单个线缆夹".into(),
            mode: mode.into(),
            provider: "demo".into(),
            model: "demo-1".into(),
            aspect: ImageAspect::Square,
        })
        .unwrap()
    }

    #[test]
    fn a_board_starts_with_the_defaults_of_its_purpose() {
        let t = TempDb::new();
        let b = t.create_board(" 线缆夹主图 ", ImagePurpose::Cover, None).unwrap();
        assert_eq!((b.name.as_str(), b.purpose, b.aspect), ("线缆夹主图", ImagePurpose::Cover, ImageAspect::Portrait));
        assert!(b.current_image_id.is_none() && b.ref_asset_ids.is_empty());
        assert!(t.create_board("  ", ImagePurpose::Free, None).is_err());

        let b = t.set_board_options(&b.id, ImagePurpose::Scene, ImageAspect::Wide).unwrap();
        assert_eq!((b.purpose, b.aspect), (ImagePurpose::Scene, ImageAspect::Wide));
        let b = t.set_board_refs(&b.id, &["ref-1".to_string()]).unwrap();
        assert_eq!(b.ref_asset_ids, ["ref-1"]);
    }

    #[test]
    fn images_form_a_tree_and_the_current_one_is_the_cover() {
        let t = TempDb::new();
        let b = t.create_board("夹子", ImagePurpose::ModelRef, None).unwrap();
        let first = image(&t, &b.id, None, "generate");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let edited = image(&t, &b.id, Some(&first.id), "edit");
        assert_eq!(edited.parent_id.as_deref(), Some(first.id.as_str()));
        assert!(!edited.adopted);

        t.set_board_current(&b.id, Some(&edited.id)).unwrap();
        let list = t.list_boards().unwrap();
        assert_eq!(list[0].cover_asset_id.as_deref(), Some(edited.asset_id.as_str()));
        assert_eq!(list[0].images, 2);
        assert_eq!(t.list_image_versions(&b.id).unwrap()[0].id, edited.id, "最新的在前");

        assert!(t.set_image_adopted(&first.id, true).unwrap().adopted);
        // 别的画板的图不能被选为当前
        let other = t.create_board("另一个", ImagePurpose::Free, None).unwrap();
        assert!(matches!(t.set_board_current(&other.id, Some(&first.id)), Err(DbError::Invalid(_))));
    }

    #[test]
    fn removing_the_current_image_falls_back_to_the_newest_one_left() {
        let t = TempDb::new();
        let b = t.create_board("夹子", ImagePurpose::ModelRef, None).unwrap();
        let first = image(&t, &b.id, None, "generate");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let second = image(&t, &b.id, Some(&first.id), "edit");
        t.set_board_current(&b.id, Some(&second.id)).unwrap();

        let after = t.remove_image_version(&second.id).unwrap();
        assert_eq!(after.current_image_id.as_deref(), Some(first.id.as_str()), "当前这张被拿掉 → 改选剩下的最新一张");
        assert_eq!(t.list_image_versions(&b.id).unwrap().len(), 1);
        assert_eq!(t.list_boards().unwrap()[0].images, 1, "列表里的张数不算拿掉的");
        assert!(matches!(t.get_image_version(&second.id), Err(DbError::NotFound(_))));
        assert!(matches!(t.set_board_current(&b.id, Some(&second.id)), Err(DbError::Invalid(_))), "拿掉的图不能再被选中");

        let empty = t.remove_image_version(&first.id).unwrap();
        assert!(empty.current_image_id.is_none(), "一张都不剩 → 没有当前图");
        assert!(t.list_boards().unwrap()[0].cover_asset_id.is_none());
        assert!(matches!(t.remove_image_version(&first.id), Err(DbError::NotFound(_))), "不能拿掉两次");
    }

    #[test]
    fn the_timeline_keeps_order_prompts_and_attachments() {
        let t = TempDb::new();
        let b = t.create_board("夹子", ImagePurpose::ModelRef, None).unwrap();
        let ask = NewImageMessage {
            board_id: b.id.clone(),
            from_user: true,
            kind: ImageMsgKind::Text,
            content: "做一张线缆夹的白底图".into(),
            extra: ImageMsgExtra::default(),
            image_asset_ids: vec!["ref-1".into()],
        };
        t.insert_image_message(&ask).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let v = image(&t, &b.id, None, "generate");
        let mut extra = ImageMsgExtra::default();
        extra.prompt = Some("白底,单个线缆夹".into());
        extra.version_ids = vec![v.id.clone()];
        t.insert_image_message(&NewImageMessage {
            board_id: b.id.clone(),
            from_user: false,
            kind: ImageMsgKind::Images,
            content: "出了 1 张".into(),
            extra,
            image_asset_ids: vec![],
        })
        .unwrap();

        let timeline = t.list_image_messages(&b.id).unwrap();
        assert_eq!(timeline.len(), 2);
        assert!(timeline[0].from_user && timeline[0].image_asset_ids == ["ref-1"]);
        assert_eq!(timeline[1].kind, ImageMsgKind::Images);
        assert_eq!(timeline[1].extra.version_ids, [v.id]);
        assert!(t.get_board(&b.id).unwrap().updated_at >= timeline[1].created_at);

        t.delete_board(&b.id).unwrap();
        assert!(t.list_boards().unwrap().is_empty());
    }
}
