//! 上架与发布(M5):笔记草稿(`content_posts`)、商品草稿(`listings`)、发布包(`publish_packs`)。
//!
//! 两张草稿表 v1 就建好了,一直没用上;v6 给它们补了几列,并新增发布包这张表。
//! 出过的发布包都留着——再出一次不覆盖旧的(和「AI 产出默认是候选、再生成不覆盖旧版本」同一个原则)。

use pp_common::publish::{Channel, DraftKind, DraftStatus, ListingDraft, LintIssue, ModelLicense, NoteAngle, NoteDraft, PublishPack, ShipMode, Sku};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::{new_id, now_ms, Db, DbError, DbResult};

const BRAND_VOICE_KEY: &str = "brand_voice";
/// 一个项目最多留多少篇笔记草稿(防止 AI 起草被连点把库塞满)
const MAX_NOTES_PER_PROJECT: i64 = 60;

fn json_err(e: serde_json::Error) -> DbError {
    DbError::Invalid(format!("json: {e}"))
}

fn list<T: for<'de> Deserialize<'de> + Default>(json: Option<String>) -> T {
    json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}

/// 商品草稿里放不进现成列的那几样。
#[derive(Debug, Default, Serialize, Deserialize)]
struct ListingExtra {
    #[serde(default)]
    selling_points: Vec<String>,
    #[serde(default)]
    ship: ShipMode,
    #[serde(default)]
    model_license: Option<ModelLicense>,
}

const NOTE_COLS: &str = "id, project_id, channel, angle, title, body, tags_json, media_json, status, external_url, ai_drafted, created_at, updated_at";

fn note_from(r: &Row<'_>) -> rusqlite::Result<NoteDraft> {
    Ok(NoteDraft {
        id: r.get(0)?,
        project_id: r.get(1)?,
        channel: Channel::parse(&r.get::<_, String>(2)?),
        angle: NoteAngle::parse(&r.get::<_, String>(3)?),
        title: r.get(4)?,
        body: r.get(5)?,
        tags: list(r.get(6)?),
        images: list(r.get(7)?),
        status: DraftStatus::parse(&r.get::<_, String>(8)?),
        external_url: r.get(9)?,
        ai_drafted: r.get::<_, Option<i64>>(10)?.unwrap_or(0) != 0,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

const LISTING_COLS: &str = "id, project_id, channel, title, body, sku_json, price, media_json, status, external_url, extra_json, created_at, updated_at";

fn listing_from(r: &Row<'_>) -> rusqlite::Result<ListingDraft> {
    let extra: ListingExtra = list(r.get(10)?);
    Ok(ListingDraft {
        id: r.get(0)?,
        project_id: r.get(1)?,
        channel: Channel::parse(&r.get::<_, String>(2)?),
        title: r.get(3)?,
        body: r.get(4)?,
        skus: list::<Vec<Sku>>(r.get(5)?),
        // 库里存的是分
        price_yuan: r.get::<_, Option<i64>>(6)?.map(|fen| fen as f64 / 100.0),
        images: list(r.get(7)?),
        status: DraftStatus::parse(&r.get::<_, String>(8)?),
        external_url: r.get(9)?,
        selling_points: extra.selling_points,
        ship: extra.ship,
        model_license: extra.model_license,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

const PACK_COLS: &str = "id, project_id, kind, draft_id, channel, dir, images_json, title, body, tags_json, lint_json, override_reason, external_url, created_at";

fn pack_from(r: &Row<'_>) -> rusqlite::Result<PublishPack> {
    Ok(PublishPack {
        id: r.get(0)?,
        project_id: r.get(1)?,
        kind: r.get(2)?,
        draft_id: r.get(3)?,
        channel: Channel::parse(&r.get::<_, String>(4)?),
        dir: r.get(5)?,
        images: list(r.get(6)?),
        title: r.get(7)?,
        body: r.get(8)?,
        tags: list(r.get(9)?),
        issues: list::<Vec<LintIssue>>(r.get(10)?),
        override_reason: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
        external_url: r.get(12)?,
        created_at: r.get(13)?,
    })
}

impl Db {
    // ---------------------------------------------------------------- 笔记草稿

    /// 最新改过的在前。
    pub fn list_notes(&self, project_id: &str) -> DbResult<Vec<NoteDraft>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!(
            "SELECT {NOTE_COLS} FROM content_posts WHERE project_id = ?1 AND deleted_at IS NULL ORDER BY updated_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([project_id], note_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_note(&self, id: &str) -> DbResult<NoteDraft> {
        self.r()
            .query_row(&format!("SELECT {NOTE_COLS} FROM content_posts WHERE id = ?1 AND deleted_at IS NULL"), [id], note_from)
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("note {id}")))
    }

    /// 新建(`id` 为空)或保存一篇笔记。状态、链接不从这里改(见 `mark_published`)。
    pub fn save_note(&self, draft: &NoteDraft) -> DbResult<NoteDraft> {
        self.get_project(&draft.project_id)?;
        let (tags, images) = (serde_json::to_string(&draft.tags).map_err(json_err)?, serde_json::to_string(&draft.images).map_err(json_err)?);
        let now = now_ms();
        if draft.id.is_empty() {
            let count: i64 = self.r().query_row("SELECT COUNT(*) FROM content_posts WHERE project_id = ?1 AND deleted_at IS NULL", [&draft.project_id], |r| r.get(0))?;
            if count >= MAX_NOTES_PER_PROJECT {
                return Err(DbError::Invalid(format!("too many note drafts in this project (max {MAX_NOTES_PER_PROJECT})")));
            }
            let id = new_id();
            self.w().execute(
                "INSERT INTO content_posts(id, project_id, channel, angle, title, body, tags_json, media_json, status, ai_drafted, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'draft', ?9, ?10, ?10)",
                params![id, draft.project_id, draft.channel.as_str(), draft.angle.as_str(), draft.title, draft.body, tags, images, draft.ai_drafted as i64, now],
            )?;
            return self.get_note(&id);
        }
        let changed = self.w().execute(
            "UPDATE content_posts SET angle = ?2, title = ?3, body = ?4, tags_json = ?5, media_json = ?6, updated_at = ?7 WHERE id = ?1 AND deleted_at IS NULL",
            params![draft.id, draft.angle.as_str(), draft.title, draft.body, tags, images, now],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound(format!("note {}", draft.id)));
        }
        self.get_note(&draft.id)
    }

    /// 软删除(出过的发布包还留着,它们自己带着当时的文案)。
    pub fn delete_note(&self, id: &str) -> DbResult<()> {
        let changed = self.w().execute("UPDATE content_posts SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL", params![id, now_ms()])?;
        if changed == 0 {
            return Err(DbError::NotFound(format!("note {id}")));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- 商品草稿

    /// 一个项目在一个渠道上一条商品;还没有就给一份空的(`id` 为空)。
    pub fn listing_of(&self, project_id: &str, channel: Channel) -> DbResult<ListingDraft> {
        self.get_project(project_id)?;
        let found = self
            .r()
            .query_row(
                &format!("SELECT {LISTING_COLS} FROM listings WHERE project_id = ?1 AND channel = ?2 AND deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1"),
                params![project_id, channel.as_str()],
                listing_from,
            )
            .optional()?;
        Ok(found.unwrap_or_else(|| ListingDraft {
            project_id: project_id.to_string(),
            channel,
            ..Default::default()
        }))
    }

    pub fn get_listing(&self, id: &str) -> DbResult<ListingDraft> {
        self.r()
            .query_row(&format!("SELECT {LISTING_COLS} FROM listings WHERE id = ?1 AND deleted_at IS NULL"), [id], listing_from)
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("listing {id}")))
    }

    pub fn save_listing(&self, draft: &ListingDraft) -> DbResult<ListingDraft> {
        self.get_project(&draft.project_id)?;
        if draft.price_yuan.is_some_and(|p| !p.is_finite() || p < 0.0) || draft.skus.iter().any(|s| !s.price_yuan.is_finite() || s.price_yuan < 0.0) {
            return Err(DbError::Invalid("prices must be non-negative numbers".into()));
        }
        let extra = ListingExtra {
            selling_points: draft.selling_points.clone(),
            ship: draft.ship,
            model_license: draft.model_license,
        };
        let (skus, images, extra) = (
            serde_json::to_string(&draft.skus).map_err(json_err)?,
            serde_json::to_string(&draft.images).map_err(json_err)?,
            serde_json::to_string(&extra).map_err(json_err)?,
        );
        let price_fen = draft.price_yuan.map(|p| (p * 100.0).round() as i64);
        let now = now_ms();
        // 一个项目一个渠道只有一条:没有 id 的保存先看看是不是已经有了
        let existing = if draft.id.is_empty() { self.listing_of(&draft.project_id, draft.channel)?.id } else { draft.id.clone() };
        if existing.is_empty() {
            let id = new_id();
            self.w().execute(
                "INSERT INTO listings(id, project_id, channel, status, title, body, sku_json, price, media_json, extra_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'draft', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![id, draft.project_id, draft.channel.as_str(), draft.title, draft.body, skus, price_fen, images, extra, now],
            )?;
            return self.get_listing(&id);
        }
        let changed = self.w().execute(
            "UPDATE listings SET title = ?2, body = ?3, sku_json = ?4, price = ?5, media_json = ?6, extra_json = ?7, updated_at = ?8 WHERE id = ?1 AND deleted_at IS NULL",
            params![existing, draft.title, draft.body, skus, price_fen, images, extra, now],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound(format!("listing {existing}")));
        }
        self.get_listing(&existing)
    }

    // ---------------------------------------------------------------- 发布包

    /// 记一个发布包,并把对应的草稿标成「出过包了」(已经发布的不往回改)。
    pub fn insert_pack(&self, pack: &PublishPack) -> DbResult<PublishPack> {
        let kind = DraftKind::parse(&pack.kind);
        let id = if pack.id.is_empty() { new_id() } else { pack.id.clone() };
        self.w().execute(
            "INSERT INTO publish_packs(id, project_id, kind, draft_id, channel, dir, images_json, title, body, tags_json, lint_json, override_reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                pack.project_id,
                kind.as_str(),
                pack.draft_id,
                pack.channel.as_str(),
                pack.dir,
                serde_json::to_string(&pack.images).map_err(json_err)?,
                pack.title,
                pack.body,
                serde_json::to_string(&pack.tags).map_err(json_err)?,
                serde_json::to_string(&pack.issues).map_err(json_err)?,
                pack.override_reason,
                now_ms(),
            ],
        )?;
        let table = if kind == DraftKind::Listing { "listings" } else { "content_posts" };
        self.w().execute(&format!("UPDATE {table} SET status = 'packed' WHERE id = ?1 AND status = 'draft'"), [&pack.draft_id])?;
        self.get_pack(&id)
    }

    pub fn get_pack(&self, id: &str) -> DbResult<PublishPack> {
        self.r()
            .query_row(&format!("SELECT {PACK_COLS} FROM publish_packs WHERE id = ?1 AND deleted_at IS NULL"), [id], pack_from)
            .optional()?
            .ok_or_else(|| DbError::NotFound(format!("publish pack {id}")))
    }

    /// 最新的在前。
    pub fn list_packs(&self, project_id: &str) -> DbResult<Vec<PublishPack>> {
        let conn = self.r();
        let mut stmt = conn.prepare(&format!("SELECT {PACK_COLS} FROM publish_packs WHERE project_id = ?1 AND deleted_at IS NULL ORDER BY created_at DESC, id DESC"))?;
        let rows = stmt.query_map([project_id], pack_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// 回填发布后的链接:这个包、以及它对应的草稿都标成「已发布」。链接可以再改(贴错了)。
    pub fn mark_published(&self, pack_id: &str, url: &str) -> DbResult<PublishPack> {
        let url = url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2000 {
            return Err(DbError::Invalid("the link must start with http:// or https://".into()));
        }
        let pack = self.get_pack(pack_id)?;
        let now = now_ms();
        self.w().execute("UPDATE publish_packs SET external_url = ?2 WHERE id = ?1", params![pack_id, url])?;
        let table = if DraftKind::parse(&pack.kind) == DraftKind::Listing { "listings" } else { "content_posts" };
        self.w().execute(
            &format!("UPDATE {table} SET status = 'published', external_url = ?2, published_at = COALESCE(published_at, ?3), updated_at = ?3 WHERE id = ?1"),
            params![pack.draft_id, url, now],
        )?;
        self.get_pack(pack_id)
    }

    // ---------------------------------------------------------------- 品牌语气

    /// 写文案时的语气(资料库级)。没设过就是空串——提示词里有一份克制的默认语气。
    pub fn brand_voice(&self) -> DbResult<String> {
        let json: Option<String> = self.r().query_row("SELECT value_json FROM library_settings WHERE key = ?1", [BRAND_VOICE_KEY], |r| r.get(0)).optional()?;
        Ok(json.and_then(|j| serde_json::from_str::<String>(&j).ok()).unwrap_or_default())
    }

    pub fn set_brand_voice(&self, voice: &str) -> DbResult<()> {
        let voice: String = voice.trim().chars().take(500).collect();
        self.w().execute(
            "INSERT INTO library_settings(key, value_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = ?2, updated_at = ?3",
            params![BRAND_VOICE_KEY, serde_json::to_string(&voice).map_err(json_err)?, now_ms()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDb;
    use pp_common::NewProject;

    fn project(t: &TempDb) -> String {
        t.create_project(&NewProject {
            title: "磁吸线缆夹".into(),
            ..Default::default()
        })
        .unwrap()
        .id
    }

    fn note(project_id: &str) -> NoteDraft {
        NoteDraft {
            project_id: project_id.into(),
            angle: NoteAngle::Scene,
            title: "桌面乱线终结者".into(),
            body: "三道线槽,磁吸底座。".into(),
            tags: vec!["桌搭".into(), "3D打印".into()],
            images: vec!["asset-1".into(), "asset-2".into()],
            ai_drafted: true,
            ..Default::default()
        }
    }

    #[test]
    fn notes_round_trip_update_and_soft_delete() {
        let t = TempDb::new();
        let pid = project(&t);
        let saved = t.save_note(&note(&pid)).unwrap();
        assert!(!saved.id.is_empty() && saved.status == DraftStatus::Draft && saved.ai_drafted);
        assert_eq!((saved.angle, saved.tags.len(), saved.images.len()), (NoteAngle::Scene, 2, 2));

        let mut edited = saved.clone();
        edited.title = "磁吸线缆夹|桌面理线".into();
        edited.images.reverse();
        let again = t.save_note(&edited).unwrap();
        assert_eq!((again.id.as_str(), again.title.as_str(), again.images[0].as_str()), (saved.id.as_str(), "磁吸线缆夹|桌面理线", "asset-2"));
        assert_eq!(t.list_notes(&pid).unwrap().len(), 1);

        t.delete_note(&saved.id).unwrap();
        assert!(t.list_notes(&pid).unwrap().is_empty());
        assert!(matches!(t.get_note(&saved.id), Err(DbError::NotFound(_))));
        assert!(matches!(t.save_note(&NoteDraft { project_id: "nope".into(), ..Default::default() }), Err(DbError::NotFound(_))));
    }

    #[test]
    fn a_project_has_one_listing_per_channel_and_prices_are_stored_in_fen() {
        let t = TempDb::new();
        let pid = project(&t);
        let empty = t.listing_of(&pid, Channel::Xhs).unwrap();
        assert!(empty.id.is_empty() && empty.project_id == pid);

        let draft = ListingDraft {
            title: "磁吸线缆夹 三槽".into(),
            selling_points: vec!["三道线槽".into(), "磁吸底座".into()],
            price_yuan: Some(29.9),
            skus: vec![Sku { name: "白色".into(), price_yuan: 29.9 }, Sku { name: "刻字".into(), price_yuan: 39.9 }],
            ship: ShipMode::Presale { days: 7 },
            model_license: Some(ModelLicense::Original),
            ..empty
        };
        let saved = t.save_listing(&draft).unwrap();
        assert_eq!(saved.price_yuan, Some(29.9));
        assert_eq!((saved.skus.len(), saved.ship, saved.model_license), (2, ShipMode::Presale { days: 7 }, Some(ModelLicense::Original)));

        // 再存一份没带 id 的:还是同一条,不会多出一条
        let second = t.save_listing(&ListingDraft { title: "改个标题".into(), ..draft.clone() }).unwrap();
        assert_eq!(second.id, saved.id);
        assert_eq!(t.listing_of(&pid, Channel::Xhs).unwrap().title, "改个标题");
        assert!(matches!(t.save_listing(&ListingDraft { price_yuan: Some(f64::NAN), ..draft }), Err(DbError::Invalid(_))));
    }

    #[test]
    fn packs_are_kept_and_publishing_marks_both_the_pack_and_its_draft() {
        let t = TempDb::new();
        let pid = project(&t);
        let n = t.save_note(&note(&pid)).unwrap();
        let pack = PublishPack {
            project_id: pid.clone(),
            kind: "note".into(),
            draft_id: n.id.clone(),
            dir: "packs/PP-0001/x".into(),
            images: vec!["01.jpg".into()],
            title: n.title.clone(),
            body: n.body.clone(),
            tags: n.tags.clone(),
            override_reason: "极限词是品牌名的一部分".into(),
            ..Default::default()
        };
        let first = t.insert_pack(&pack).unwrap();
        assert_eq!(t.get_note(&n.id).unwrap().status, DraftStatus::Packed);
        let _second = t.insert_pack(&pack).unwrap();
        assert_eq!(t.list_packs(&pid).unwrap().len(), 2, "再出一次不覆盖旧的");
        assert_eq!(first.override_reason, "极限词是品牌名的一部分");

        assert!(matches!(t.mark_published(&first.id, "javascript:alert(1)"), Err(DbError::Invalid(_))));
        let done = t.mark_published(&first.id, " https://www.xiaohongshu.com/explore/abc ").unwrap();
        assert_eq!(done.external_url.as_deref(), Some("https://www.xiaohongshu.com/explore/abc"));
        let published = t.get_note(&n.id).unwrap();
        assert_eq!((published.status, published.external_url.as_deref()), (DraftStatus::Published, Some("https://www.xiaohongshu.com/explore/abc")));

        let facts = t.project_facts(&pid).unwrap();
        assert_eq!((facts.publish_packs, facts.published), (2, 1));
    }

    #[test]
    fn brand_voice_is_a_library_setting() {
        let t = TempDb::new();
        assert_eq!(t.brand_voice().unwrap(), "");
        t.set_brand_voice("  简洁、真诚、不夸张  ").unwrap();
        assert_eq!(t.brand_voice().unwrap(), "简洁、真诚、不夸张");
    }
}
