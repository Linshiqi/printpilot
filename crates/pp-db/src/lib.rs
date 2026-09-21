//! SQLite 数据层。
//!
//! 连接策略照搬 velo:WAL + `synchronous=NORMAL` + `busy_timeout=5000`,**读写各一条连接**
//! (WAL 下读不阻塞写,界面查询不会被后台任务的写事务卡住)。

mod assets;
mod cad;
mod costs;
mod designs;
mod imagery;
mod overview;
mod pricing;
mod projects;
mod research;
pub mod schema;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use pp_common::errcode;
use rusqlite::Connection;

pub use assets::NewAsset;
pub use cad::NewCadVersion;
pub use designs::NewCadMessage;
pub use imagery::{NewImageMessage, NewImageVersion};
pub use schema::SCHEMA_VERSION;

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    NotFound(String),
    Invalid(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "{e}"),
            DbError::NotFound(what) => write!(f, "{what}"),
            DbError::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

impl DbError {
    /// 转成命令层的 `#错误码#细节` 约定。
    pub fn to_wire(&self) -> String {
        match self {
            DbError::Sqlite(e) => errcode::err(errcode::DB_FAILED, e),
            DbError::NotFound(what) => errcode::err(errcode::NOT_FOUND, what),
            DbError::Invalid(why) => errcode::err(errcode::INVALID_INPUT, why),
        }
    }
}

pub type DbResult<T> = Result<T, DbError>;

pub struct Db {
    write: Mutex<Connection>,
    read: Mutex<Connection>,
    path: PathBuf,
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        let write = Connection::open(path)?;
        configure(&write)?;
        schema::migrate(&write)?;
        let read = Connection::open(path)?;
        configure(&read)?;
        Ok(Self {
            write: Mutex::new(write),
            read: Mutex::new(read),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> DbResult<u32> {
        Ok(self
            .r()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    // 锁中毒 = 某次持锁时 panic 了。连接本身仍然可用(SQLite 事务会回滚),继续用比整个应用瘫掉好。
    fn w(&self) -> MutexGuard<'_, Connection> {
        self.write.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn r(&self) -> MutexGuard<'_, Connection> {
        self.read.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// 每个测试一个独立的临时库文件(读写两条连接要看到同一份数据,内存库做不到)。
    pub struct TempDb {
        pub db: Db,
        dir: PathBuf,
    }

    impl TempDb {
        pub fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("pp-db-test-{}", new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let db = Db::open(&dir.join("printpilot.db")).unwrap();
            Self { db, dir }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl std::ops::Deref for TempDb {
        type Target = Db;
        fn deref(&self) -> &Db {
            &self.db
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::TempDb;
    use super::*;

    #[test]
    fn opens_in_wal_mode_at_current_schema() {
        let t = TempDb::new();
        assert_eq!(t.schema_version().unwrap(), SCHEMA_VERSION);
        let mode: String = t
            .r()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn reopening_keeps_data() {
        let t = TempDb::new();
        let p = t
            .create_project(&pp_common::NewProject {
                title: "磁吸线缆夹".into(),
                ..Default::default()
            })
            .unwrap();
        let path = t.path().to_path_buf();
        let again = Db::open(&path).unwrap();
        assert_eq!(again.get_project(&p.id).unwrap().title, "磁吸线缆夹");
    }

    #[test]
    fn wire_errors_carry_codes() {
        assert_eq!(
            DbError::NotFound("project x".into()).to_wire(),
            "#not_found#project x"
        );
        assert!(DbError::Invalid("空标题".into())
            .to_wire()
            .starts_with("#invalid_input#"));
    }
}
