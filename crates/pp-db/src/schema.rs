//! 建表与迁移(做法照搬 velo):
//! - `PRAGMA user_version` 记版本,一串 `if ver < N { … }` 线性脚本,末尾写回版本号;
//! - 建表一律 `CREATE TABLE IF NOT EXISTS` 兜底;
//! - 不能用版本号表达的一次性数据回填,用 `meta` 表打标(`migration_done` / `mark_migration_done`)。
//!
//! 约定:主键 UUIDv7 文本;时间 = 毫秒整数;金额 = 整数「分」;灵活结构存 JSON 文本;
//! 软删除 `deleted_at`(为将来的同步留余地)。

use rusqlite::Connection;

/// 每加一段迁移就 +1。**已发布版本的迁移段不可修改**,只能追加。
pub const SCHEMA_VERSION: u32 = 4;

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let ver: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if ver > SCHEMA_VERSION {
        // 用新版本打开过的库被旧版本程序打开:不动它,只告警。表结构向后兼容是迁移脚本的责任。
        log::warn!("[db] 数据库版本 {ver} 高于程序支持的 {SCHEMA_VERSION},按只增不改的约定继续使用");
        return Ok(());
    }
    if ver < 1 {
        conn.execute_batch(V1)?;
    }
    if ver < 2 {
        conn.execute_batch(V2)?;
    }
    if ver < 3 {
        conn.execute_batch(V3)?;
    }
    if ver < 4 {
        conn.execute_batch(V4)?;
    }
    if ver != SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        log::info!("[db] schema {ver} → {SCHEMA_VERSION}");
    }
    Ok(())
}

/// v1:MVP-α 需要的表。订单、指标快照、实验、经验库等 MVP-β 的表到时用 v2 追加。
const V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id               TEXT PRIMARY KEY,
    code             TEXT NOT NULL UNIQUE,
    title            TEXT NOT NULL,
    stage            TEXT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'active',
    category         TEXT NOT NULL DEFAULT '',
    hypothesis       TEXT NOT NULL DEFAULT '',
    score_json       TEXT,
    kill_reason      TEXT,
    cover_asset_id   TEXT,
    stage_entered_at INTEGER NOT NULL,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    deleted_at       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_projects_stage ON projects(stage) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS stage_events (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    from_stage TEXT,
    to_stage   TEXT NOT NULL,
    actor      TEXT NOT NULL DEFAULT 'user',
    forced     INTEGER NOT NULL DEFAULT 0,
    note       TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_stage_events_project ON stage_events(project_id, created_at);

CREATE TABLE IF NOT EXISTS research_runs (
    id           TEXT PRIMARY KEY,
    project_id   TEXT REFERENCES projects(id),
    query        TEXT NOT NULL,
    params_json  TEXT NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'queued',
    report_md    TEXT NOT NULL DEFAULT '',
    sources_json TEXT NOT NULL DEFAULT '[]',
    cost         INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    finished_at  INTEGER
);

CREATE TABLE IF NOT EXISTS opportunities (
    id                 TEXT PRIMARY KEY,
    research_run_id    TEXT NOT NULL REFERENCES research_runs(id),
    title              TEXT NOT NULL,
    summary            TEXT NOT NULL DEFAULT '',
    scores_json        TEXT NOT NULL DEFAULT '{}',
    total_score        REAL NOT NULL DEFAULT 0,
    detail_json        TEXT NOT NULL DEFAULT '{}',
    adopted_project_id TEXT REFERENCES projects(id),
    created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_opportunities_run ON opportunities(research_run_id);

CREATE TABLE IF NOT EXISTS assets (
    id              TEXT PRIMARY KEY,
    project_id      TEXT REFERENCES projects(id),
    kind            TEXT NOT NULL,
    role            TEXT NOT NULL DEFAULT '',
    rel_path        TEXT NOT NULL,
    ext             TEXT NOT NULL DEFAULT '',
    bytes           INTEGER NOT NULL DEFAULT 0,
    meta_json       TEXT,
    parent_asset_id TEXT REFERENCES assets(id),
    source_job_id   TEXT,
    ai_generated    INTEGER NOT NULL DEFAULT 0,
    is_adopted      INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    deleted_at      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_assets_project ON assets(project_id, created_at) WHERE deleted_at IS NULL;

-- 任务:调度器直接读写这张表(状态机见 docs/03-architecture.md §6)
CREATE TABLE IF NOT EXISTS jobs (
    id               TEXT PRIMARY KEY,
    project_id       TEXT REFERENCES projects(id),
    type             TEXT NOT NULL,
    provider         TEXT NOT NULL DEFAULT '',
    provider_task_id TEXT,
    status           TEXT NOT NULL DEFAULT 'queued',
    input_json       TEXT NOT NULL DEFAULT '{}',
    output_json      TEXT,
    error_code       TEXT,
    error_detail     TEXT,
    attempts         INTEGER NOT NULL DEFAULT 0,
    max_attempts     INTEGER NOT NULL DEFAULT 3,
    run_after        INTEGER NOT NULL DEFAULT 0,
    dedupe_key       TEXT,
    progress         REAL NOT NULL DEFAULT 0,
    est_cost         INTEGER NOT NULL DEFAULT 0,
    cost             INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    started_at       INTEGER,
    finished_at      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_jobs_due ON jobs(status, run_after);
CREATE INDEX IF NOT EXISTS idx_jobs_project ON jobs(project_id, created_at);
-- 同一输入的进行中任务只允许一个(防连点重复扣费);结束后的任务不占用这个键
CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_dedupe ON jobs(dedupe_key)
    WHERE dedupe_key IS NOT NULL AND status IN ('queued', 'running', 'waiting_provider', 'persisting');

CREATE TABLE IF NOT EXISTS cost_entries (
    id         TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(id),
    job_id     TEXT REFERENCES jobs(id),
    category   TEXT NOT NULL,
    amount     INTEGER NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cost_project ON cost_entries(project_id);
CREATE INDEX IF NOT EXISTS idx_cost_time ON cost_entries(created_at);

CREATE TABLE IF NOT EXISTS printers (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    model          TEXT NOT NULL DEFAULT '',
    build_x        REAL NOT NULL DEFAULT 256,
    build_y        REAL NOT NULL DEFAULT 256,
    build_z        REAL NOT NULL DEFAULT 256,
    power_w        REAL NOT NULL DEFAULT 100,
    price          INTEGER NOT NULL DEFAULT 0,
    lifetime_hours REAL NOT NULL DEFAULT 5000,
    grams_per_hour REAL NOT NULL DEFAULT 28,
    created_at     INTEGER NOT NULL,
    deleted_at     INTEGER
);

CREATE TABLE IF NOT EXISTS materials (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    type        TEXT NOT NULL DEFAULT 'PLA',
    color       TEXT NOT NULL DEFAULT '',
    cost_per_kg INTEGER NOT NULL DEFAULT 0,
    density     REAL NOT NULL DEFAULT 1.24,
    stock_g     REAL NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);

CREATE TABLE IF NOT EXISTS print_runs (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id),
    asset_id       TEXT REFERENCES assets(id),
    printer_id     TEXT REFERENCES printers(id),
    material_id    TEXT REFERENCES materials(id),
    settings_json  TEXT NOT NULL DEFAULT '{}',
    est_minutes    REAL,
    est_grams      REAL,
    actual_minutes REAL,
    actual_grams   REAL,
    result         TEXT NOT NULL DEFAULT 'pending',
    fail_reason    TEXT,
    note           TEXT NOT NULL DEFAULT '',
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_print_runs_project ON print_runs(project_id, created_at);

CREATE TABLE IF NOT EXISTS cost_models (
    project_id   TEXT PRIMARY KEY REFERENCES projects(id),
    params_json  TEXT NOT NULL DEFAULT '{}',
    unit_cost    INTEGER NOT NULL DEFAULT 0,
    chosen_price INTEGER,
    updated_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS listings (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES projects(id),
    channel      TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'draft',
    title        TEXT NOT NULL DEFAULT '',
    body         TEXT NOT NULL DEFAULT '',
    sku_json     TEXT NOT NULL DEFAULT '[]',
    price        INTEGER,
    media_json   TEXT NOT NULL DEFAULT '[]',
    external_url TEXT,
    lint_json    TEXT,
    published_at INTEGER,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_listings_project ON listings(project_id);

CREATE TABLE IF NOT EXISTS content_posts (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id),
    channel        TEXT NOT NULL,
    listing_id     TEXT REFERENCES listings(id),
    angle          TEXT NOT NULL DEFAULT '',
    title          TEXT NOT NULL DEFAULT '',
    body           TEXT NOT NULL DEFAULT '',
    tags_json      TEXT NOT NULL DEFAULT '[]',
    cover_asset_id TEXT REFERENCES assets(id),
    media_json     TEXT NOT NULL DEFAULT '[]',
    variant_of     TEXT REFERENCES content_posts(id),
    experiment_id  TEXT,
    status         TEXT NOT NULL DEFAULT 'draft',
    external_url   TEXT,
    lint_json      TEXT,
    scheduled_at   INTEGER,
    published_at   INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_posts_project ON content_posts(project_id);
"#;

/// v2:代码式 CAD 的版本树(docs/adr/0003)。每次生成、改参数、指令修补都是一个新版本,`parent_id` 指向它改自哪一版。
const V2: &str = r#"
CREATE TABLE IF NOT EXISTS cad_versions (
    id            TEXT PRIMARY KEY,
    project_id    TEXT REFERENCES projects(id),
    parent_id     TEXT REFERENCES cad_versions(id),
    source        TEXT NOT NULL,
    note          TEXT NOT NULL DEFAULT '',
    code          TEXT NOT NULL,
    spec_json     TEXT,
    ref_asset_ids_json TEXT NOT NULL DEFAULT '[]',
    report_json   TEXT,
    params_json   TEXT NOT NULL DEFAULT '[]',
    metrics_json  TEXT NOT NULL,
    stl_asset_id  TEXT NOT NULL REFERENCES assets(id),
    step_asset_id TEXT REFERENCES assets(id),
    elapsed_ms    INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    deleted_at    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_cad_versions_project ON cad_versions(project_id, created_at);
"#;

/// v3:建模工作室。**设计** = 一个零件的一条建模线索(参考图、规格、对话、版本树);
/// 对话与时间线事件存在 `cad_messages`;版本通过 `design_id` 归到设计名下。
/// (v2 已随 0.9.0 发布,不能再改,所以 `design_id` 用 ALTER TABLE 追加。)
const V3: &str = r#"
CREATE TABLE IF NOT EXISTS cad_designs (
    id                 TEXT PRIMARY KEY,
    project_id         TEXT REFERENCES projects(id),
    name               TEXT NOT NULL,
    spec_json          TEXT,
    ref_asset_ids_json TEXT NOT NULL DEFAULT '[]',
    current_version_id TEXT,
    thumb              TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    deleted_at         INTEGER
);
CREATE INDEX IF NOT EXISTS idx_cad_designs_updated ON cad_designs(updated_at);

CREATE TABLE IF NOT EXISTS cad_messages (
    id                   TEXT PRIMARY KEY,
    design_id            TEXT NOT NULL REFERENCES cad_designs(id),
    role                 TEXT NOT NULL,
    kind                 TEXT NOT NULL,
    content              TEXT NOT NULL DEFAULT '',
    extra_json           TEXT NOT NULL DEFAULT '{}',
    image_asset_ids_json TEXT NOT NULL DEFAULT '[]',
    version_id           TEXT REFERENCES cad_versions(id),
    created_at           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cad_messages_design ON cad_messages(design_id, created_at);

ALTER TABLE cad_versions ADD COLUMN design_id TEXT REFERENCES cad_designs(id);
CREATE INDEX IF NOT EXISTS idx_cad_versions_design ON cad_versions(design_id, created_at);
"#;

/// v4:图片工作台。**画板** = 一个主题的一组图(参考图、对话、生成过的每一张图);
/// 和建模工作室同构:`image_boards` ↔ `cad_designs`,`image_versions` ↔ `cad_versions`,`image_messages` ↔ `cad_messages`。
const V4: &str = r#"
CREATE TABLE IF NOT EXISTS image_boards (
    id                 TEXT PRIMARY KEY,
    project_id         TEXT REFERENCES projects(id),
    name               TEXT NOT NULL,
    purpose            TEXT NOT NULL DEFAULT 'model_ref',
    aspect             TEXT NOT NULL DEFAULT 'square',
    ref_asset_ids_json TEXT NOT NULL DEFAULT '[]',
    current_image_id   TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    deleted_at         INTEGER
);
CREATE INDEX IF NOT EXISTS idx_image_boards_updated ON image_boards(updated_at);

CREATE TABLE IF NOT EXISTS image_versions (
    id         TEXT PRIMARY KEY,
    board_id   TEXT NOT NULL REFERENCES image_boards(id),
    parent_id  TEXT REFERENCES image_versions(id),
    asset_id   TEXT NOT NULL REFERENCES assets(id),
    prompt     TEXT NOT NULL DEFAULT '',
    mode       TEXT NOT NULL DEFAULT 'generate',
    provider   TEXT NOT NULL DEFAULT '',
    model      TEXT NOT NULL DEFAULT '',
    aspect     TEXT NOT NULL DEFAULT 'square',
    adopted    INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    deleted_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_image_versions_board ON image_versions(board_id, created_at);

CREATE TABLE IF NOT EXISTS image_messages (
    id                   TEXT PRIMARY KEY,
    board_id             TEXT NOT NULL REFERENCES image_boards(id),
    from_user            INTEGER NOT NULL,
    kind                 TEXT NOT NULL,
    content              TEXT NOT NULL DEFAULT '',
    extra_json           TEXT NOT NULL DEFAULT '{}',
    image_asset_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_image_messages_board ON image_messages(board_id, created_at);
"#;

/// 一次性数据回填是否做过(velo 做法)。
pub fn migration_done(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM meta WHERE key = ?1",
        [format!("migration:{name}")],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn mark_migration_done(conn: &Connection, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, '1')",
        [format!("migration:{name}")],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_lands_on_current_version_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let ver: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        // 再跑一遍不能出错,也不能改变任何东西
        migrate(&conn).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 20);
    }

    #[test]
    fn a_0_9_0_database_gains_the_studio_tables_and_keeps_its_versions() {
        // 0.9.0 发出去的库是 v2:里面可能已经有预研页建的版本
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(V2).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute(
            "INSERT INTO assets(id, kind, role, rel_path, ext, bytes, created_at) VALUES ('a1', 'model3d', 'cad_mesh', 'assets/lab/a1.stl', 'stl', 1, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cad_versions(id, source, code, metrics_json, stl_asset_id, created_at) VALUES ('v1', 'manual', 'result = 1', '{}', 'a1', 0)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let ver: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        let design_id: Option<String> = conn.query_row("SELECT design_id FROM cad_versions WHERE id = 'v1'", [], |r| r.get(0)).unwrap();
        assert_eq!(design_id, None, "旧版本留着,只是不属于任何设计");
        for table in ["cad_designs", "cad_messages"] {
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = ?1", [table], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn a_v1_database_is_upgraded_in_place_without_losing_rows() {
        // 模拟「上一个版本建的库」:只跑 v1,写一行数据,再用当前版本打开
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        conn.execute(
            "INSERT INTO projects(id, code, title, stage, stage_entered_at, created_at, updated_at)
             VALUES ('p1', 'PP-0001', '旧项目', 'idea', 0, 0, 0)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let ver: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
        let title: String = conn.query_row("SELECT title FROM projects WHERE id = 'p1'", [], |r| r.get(0)).unwrap();
        assert_eq!(title, "旧项目");
        let has_cad: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'cad_versions'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(has_cad, 1);
    }

    #[test]
    fn newer_db_is_left_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 999;").unwrap();
        migrate(&conn).unwrap();
        let ver: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, 999);
    }

    #[test]
    fn dedupe_index_blocks_second_live_job_but_not_after_finish() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let insert = |id: &str, status: &str| {
            conn.execute(
                "INSERT INTO jobs(id, type, status, dedupe_key, created_at) VALUES (?1, 'image.generate', ?2, 'k1', 0)",
                [id, status],
            )
        };
        insert("a", "queued").unwrap();
        assert!(insert("b", "queued").is_err(), "同一输入的进行中任务只能有一个");
        conn.execute("UPDATE jobs SET status = 'succeeded' WHERE id = 'a'", [])
            .unwrap();
        insert("c", "queued").expect("前一个结束后应该可以再排一个");
    }

    #[test]
    fn one_off_migration_marker() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert!(!migration_done(&conn, "backfill_x").unwrap());
        mark_migration_done(&conn, "backfill_x").unwrap();
        assert!(migration_done(&conn, "backfill_x").unwrap());
    }
}
