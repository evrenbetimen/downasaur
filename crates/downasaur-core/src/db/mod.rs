//! Embedded SQLite persistence for the queue, chunk state and settings.
//!
//! The database runs in WAL mode with `synchronous=NORMAL`: committed
//! transactions survive a crash or power loss without fsync-per-write cost, and
//! the UI can read the queue while workers write progress. Schema changes are
//! applied in order and tracked with `PRAGMA user_version`.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;
use uuid::Uuid;

use crate::downloader::Chunk;
use crate::error::{CoreError, Result};
use crate::model::{DownloadProfile, DownloadTask, Platform, TaskState};

const MIGRATIONS: &[&str] = &[
    // v1: queue, chunks, settings
    r"
    CREATE TABLE tasks (
        id           TEXT PRIMARY KEY,
        source_url   TEXT NOT NULL,
        platform     TEXT NOT NULL,
        title        TEXT NOT NULL,
        author       TEXT,
        profile      TEXT NOT NULL,
        state        TEXT NOT NULL,
        bytes_total  INTEGER,
        bytes_done   INTEGER NOT NULL DEFAULT 0,
        destination  TEXT,
        error        TEXT,
        scheduled_at TEXT,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL
    );
    CREATE INDEX tasks_state ON tasks(state);
    CREATE TABLE chunks (
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        idx     INTEGER NOT NULL,
        start   INTEGER NOT NULL,
        end     INTEGER NOT NULL,
        done    INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (task_id, idx)
    ) WITHOUT ROWID;
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
];

#[derive(Debug, Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let db = Self { conn: Arc::new(Mutex::new(conn)) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock();
        let current: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn journal_mode(&self) -> Result<String> {
        Ok(self.conn.lock().pragma_query_value(None, "journal_mode", |r| r.get(0))?)
    }

    // ---- tasks ----------------------------------------------------------

    pub fn insert_task(&self, t: &DownloadTask) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO tasks (id, source_url, platform, title, author, profile, state, bytes_total, bytes_done,
                                destination, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                t.id.to_string(),
                t.source_url.as_str(),
                to_json(&t.platform)?,
                t.title,
                t.author,
                to_json(&t.profile)?,
                t.state.as_str(),
                t.bytes_total,
                t.bytes_done,
                t.destination,
                t.error,
                t.created_at,
                t.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_state(&self, id: Uuid, state: TaskState, error: Option<&str>) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE tasks SET state = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
            params![id.to_string(), state.as_str(), error, Utc::now()],
        )?;
        Ok(())
    }

    pub fn set_progress(&self, id: Uuid, bytes_done: u64, bytes_total: Option<u64>) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE tasks SET bytes_done = ?2, bytes_total = COALESCE(?3, bytes_total), updated_at = ?4 WHERE id = ?1",
            params![id.to_string(), bytes_done, bytes_total, Utc::now()],
        )?;
        Ok(())
    }

    pub fn set_destination(&self, id: Uuid, destination: &str) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE tasks SET destination = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), destination, Utc::now()],
        )?;
        Ok(())
    }

    pub fn get_task(&self, id: Uuid) -> Result<Option<DownloadTask>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT * FROM tasks WHERE id = ?1")?;
        stmt.query_row([id.to_string()], row_to_task).optional().map_err(Into::into)
    }

    pub fn list_tasks(&self) -> Result<Vec<DownloadTask>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT * FROM tasks ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], row_to_task)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Tasks interrupted by a shutdown: anything not terminal and not paused.
    pub fn interrupted_tasks(&self) -> Result<Vec<DownloadTask>> {
        Ok(self
            .list_tasks()?
            .into_iter()
            .filter(|t| !t.state.is_terminal() && !matches!(t.state, TaskState::Paused | TaskState::Scheduled))
            .collect())
    }

    pub fn delete_task(&self, id: Uuid) -> Result<()> {
        self.conn.lock().execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    // ---- chunks ---------------------------------------------------------

    pub fn save_chunks(&self, task: Uuid, chunks: &[Chunk]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO chunks (task_id, idx, start, end, done) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(task_id, idx) DO UPDATE SET done = excluded.done",
            )?;
            for c in chunks {
                stmt.execute(params![task.to_string(), c.index, c.start, c.end, c.done])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_chunk(&self, task: Uuid, chunk: &Chunk) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE chunks SET done = ?3 WHERE task_id = ?1 AND idx = ?2",
            params![task.to_string(), chunk.index, chunk.done],
        )?;
        Ok(())
    }

    pub fn load_chunks(&self, task: Uuid) -> Result<Vec<Chunk>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("SELECT idx, start, end, done FROM chunks WHERE task_id = ?1 ORDER BY idx")?;
        let rows = stmt.query_map([task.to_string()], |r| {
            Ok(Chunk { index: r.get(0)?, start: r.get(1)?, end: r.get(2)?, done: r.get(3)? })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn clear_chunks(&self, task: Uuid) -> Result<()> {
        self.conn.lock().execute("DELETE FROM chunks WHERE task_id = ?1", [task.to_string()])?;
        Ok(())
    }

    // ---- settings -------------------------------------------------------

    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw: Option<String> =
            self.conn.lock().query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(|e| CoreError::Parse(e.to_string()))).transpose()
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, to_json(value)?],
        )?;
        Ok(())
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| CoreError::Parse(e.to_string()))
}

fn json_col<T: DeserializeOwned>(row: &Row<'_>, col: &str) -> rusqlite::Result<T> {
    let s: String = row.get(col)?;
    serde_json::from_str(&s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

fn row_to_task(r: &Row<'_>) -> rusqlite::Result<DownloadTask> {
    let conv = |e: Box<dyn std::error::Error + Send + Sync>| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e)
    };
    let id: String = r.get("id")?;
    let url: String = r.get("source_url")?;
    let state: String = r.get("state")?;
    Ok(DownloadTask {
        id: Uuid::parse_str(&id).map_err(|e| conv(Box::new(e)))?,
        source_url: Url::parse(&url).map_err(|e| conv(Box::new(e)))?,
        platform: json_col::<Platform>(r, "platform")?,
        title: r.get("title")?,
        author: r.get("author")?,
        profile: json_col::<DownloadProfile>(r, "profile")?,
        state: TaskState::parse(&state).ok_or_else(|| conv(format!("unknown state `{state}`").into()))?,
        bytes_total: r.get("bytes_total")?,
        bytes_done: r.get("bytes_done")?,
        destination: r.get("destination")?,
        error: r.get("error")?,
        created_at: r.get::<_, DateTime<Utc>>("created_at")?,
        updated_at: r.get::<_, DateTime<Utc>>("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::plan_chunks;

    fn task() -> DownloadTask {
        let now = Utc::now();
        DownloadTask {
            id: Uuid::new_v4(),
            source_url: Url::parse("https://youtu.be/abcdefghijk").expect("url"),
            platform: Platform::YouTube,
            title: "8K demo".into(),
            author: Some("Creator".into()),
            profile: DownloadProfile::ultra_8k(),
            state: TaskState::Queued,
            bytes_total: None,
            bytes_done: 0,
            destination: None,
            error: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn task_roundtrip_and_state_updates() {
        let db = Database::open_in_memory().expect("db");
        let t = task();
        db.insert_task(&t).expect("insert");
        db.set_progress(t.id, 1024, Some(4096)).expect("progress");
        db.set_state(t.id, TaskState::Downloading, None).expect("state");
        let got = db.get_task(t.id).expect("query").expect("exists");
        assert_eq!(got.profile, DownloadProfile::ultra_8k());
        assert_eq!((got.bytes_done, got.bytes_total), (1024, Some(4096)));
        assert_eq!(db.interrupted_tasks().expect("list").len(), 1);
    }

    #[test]
    fn chunk_state_survives_reload() {
        let db = Database::open_in_memory().expect("db");
        let t = task();
        db.insert_task(&t).expect("insert");
        let mut chunks = plan_chunks(1000, 300);
        db.save_chunks(t.id, &chunks).expect("save");
        chunks[1].done = 120;
        db.update_chunk(t.id, &chunks[1]).expect("update");
        let loaded = db.load_chunks(t.id).expect("load");
        assert_eq!(loaded, chunks);
        db.delete_task(t.id).expect("delete");
        assert!(db.load_chunks(t.id).expect("load").is_empty(), "chunks cascade");
    }

    #[test]
    fn wal_enabled_on_disk() {
        let path = std::env::temp_dir().join(format!("downasaur-{}.db", Uuid::new_v4()));
        let db = Database::open(&path).expect("db");
        assert_eq!(db.journal_mode().expect("mode"), "wal");
        db.set_setting("k", &vec![1, 2, 3]).expect("set");
        assert_eq!(db.get_setting::<Vec<i32>>("k").expect("get"), Some(vec![1, 2, 3]));
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
