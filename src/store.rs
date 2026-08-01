use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    AtCapacity,
    IdCollision,
    Other(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("plan not found"),
            Self::AtCapacity => f.write_str("plan limit reached"),
            Self::IdCollision => f.write_str("plan id collision"),
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Debug)]
pub struct OpenError(String);

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OpenError {}

pub struct Store {
    conn: Arc<Mutex<Connection>>,
    max_plans: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

impl Store {
    pub fn open(path: impl AsRef<Path>, max_plans: usize) -> Result<Self, OpenError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && parent != Path::new(".") {
                std::fs::create_dir_all(parent)
                    .map_err(|e| OpenError(format!("create database directory: {e}")))?;
            }
        }

        let conn = Connection::open(path).map_err(|e| OpenError(format!("open database: {e}")))?;
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             CREATE TABLE IF NOT EXISTS plans (
                id TEXT PRIMARY KEY,
                html BLOB NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'text/html; charset=utf-8',
                created_at INTEGER NOT NULL
             );",
        )
        .map_err(|e| OpenError(format!("initialize database: {e}")))?;

        let has_content_type = conn
            .prepare("PRAGMA table_info(plans)")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|e| OpenError(format!("inspect database schema: {e}")))?
            .iter()
            .any(|column| column == "content_type");
        if !has_content_type {
            conn.execute(
                "ALTER TABLE plans ADD COLUMN content_type TEXT NOT NULL DEFAULT 'text/html; charset=utf-8'",
                [],
            )
            .map_err(|e| OpenError(format!("migrate database schema: {e}")))?;
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            max_plans,
        })
    }

    pub fn create(&self, id: &str, bytes: &[u8], content_type: &str) -> Result<(), StoreError> {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("database lock poisoned".into()))?;
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| StoreError::Other(format!("begin create transaction: {e}")))?;

        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM plans", [], |row| row.get(0))
            .map_err(|e| StoreError::Other(format!("count plans: {e}")))?;
        if count as usize >= self.max_plans {
            return Err(StoreError::AtCapacity);
        }

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO plans (id, html, content_type, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, bytes, content_type, created_at],
            )
            .map_err(|e| StoreError::Other(format!("insert plan: {e}")))?;
        if changed == 0 {
            return Err(StoreError::IdCollision);
        }
        tx.commit()
            .map_err(|e| StoreError::Other(format!("commit plan: {e}")))?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Payload, StoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("database lock poisoned".into()))?;
        guard
            .query_row(
                "SELECT html, content_type FROM plans WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Payload {
                        bytes: row.get(0)?,
                        content_type: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(|e| StoreError::Other(format!("get plan: {e}")))?
            .ok_or(StoreError::NotFound)
    }

    pub fn wipe(&self) -> Result<(), StoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("database lock poisoned".into()))?;
        guard
            .execute("DELETE FROM plans", [])
            .map_err(|e| StoreError::Other(format!("delete plans: {e}")))?;
        guard
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| StoreError::Other(format!("checkpoint database: {e}")))?;
        guard
            .execute_batch("VACUUM;")
            .map_err(|e| StoreError::Other(format!("vacuum database: {e}")))?;
        Ok(())
    }

    pub fn health(&self) -> Result<(), StoreError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("database lock poisoned".into()))?;
        let one: i64 = guard
            .query_row("SELECT 1", [], |row| row.get(0))
            .map_err(|e| StoreError::Other(format!("check database: {e}")))?;
        if one != 1 {
            return Err(StoreError::Other("unexpected health result".into()));
        }
        Ok(())
    }

    pub async fn create_async(
        &self,
        id: String,
        bytes: Vec<u8>,
        content_type: String,
    ) -> Result<(), StoreError> {
        let store = Self {
            conn: Arc::clone(&self.conn),
            max_plans: self.max_plans,
        };
        tokio::task::spawn_blocking(move || store.create(&id, &bytes, &content_type))
            .await
            .map_err(|e| StoreError::Other(format!("store task panicked: {e}")))?
    }

    pub async fn get_async(&self, id: String) -> Result<Payload, StoreError> {
        let store = Self {
            conn: Arc::clone(&self.conn),
            max_plans: self.max_plans,
        };
        tokio::task::spawn_blocking(move || store.get(&id))
            .await
            .map_err(|e| StoreError::Other(format!("store task panicked: {e}")))?
    }

    pub async fn wipe_async(&self) -> Result<(), StoreError> {
        let store = Self {
            conn: Arc::clone(&self.conn),
            max_plans: self.max_plans,
        };
        tokio::task::spawn_blocking(move || store.wipe())
            .await
            .map_err(|e| StoreError::Other(format!("store task panicked: {e}")))?
    }

    pub async fn health_async(&self) -> Result<(), StoreError> {
        let store = Self {
            conn: Arc::clone(&self.conn),
            max_plans: self.max_plans,
        };
        tokio::task::spawn_blocking(move || store.health())
            .await
            .map_err(|e| StoreError::Other(format!("store task panicked: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tempfile() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "planista-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn store_persists_and_wipes() {
        let dir = tempfile();
        let path = dir.join("nested").join("planista.db");
        let store = Store::open(&path, 2).unwrap();
        store
            .create(
                "abcdefghijklmnop",
                b"<h1>persisted</h1>",
                "text/html; charset=utf-8",
            )
            .unwrap();
        drop(store);

        let store = Store::open(&path, 2).unwrap();
        assert_eq!(
            store.get("abcdefghijklmnop").unwrap(),
            Payload {
                bytes: b"<h1>persisted</h1>".to_vec(),
                content_type: "text/html; charset=utf-8".into(),
            }
        );
        store.wipe().unwrap();
        assert_eq!(store.get("abcdefghijklmnop"), Err(StoreError::NotFound));
        store.health().unwrap();
    }

    #[test]
    fn store_capacity_and_collision() {
        let dir = tempfile();
        let store = Store::open(dir.join("planista.db"), 1).unwrap();
        store
            .create("abcdefghijklmnop", b"first", "application/octet-stream")
            .unwrap();
        assert_eq!(
            store.create("differentIDvalue", b"second", "application/octet-stream"),
            Err(StoreError::AtCapacity)
        );

        let collision = Store::open(dir.join("collision.db"), 2).unwrap();
        collision
            .create("abcdefghijklmnop", b"first", "application/octet-stream")
            .unwrap();
        assert_eq!(
            collision.create("abcdefghijklmnop", b"second", "application/octet-stream"),
            Err(StoreError::IdCollision)
        );
    }

    #[test]
    fn migrates_existing_html_only_database() {
        let dir = tempfile();
        let path = dir.join("planista.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE plans (
                id TEXT PRIMARY KEY,
                html BLOB NOT NULL,
                created_at INTEGER NOT NULL
             );
             INSERT INTO plans (id, html, created_at)
             VALUES ('abcdefghijklmnop', X'3C703E6F6C643C2F703E', 0);",
        )
        .unwrap();
        drop(conn);

        let store = Store::open(&path, 2).unwrap();
        assert_eq!(
            store.get("abcdefghijklmnop").unwrap(),
            Payload {
                bytes: b"<p>old</p>".to_vec(),
                content_type: "text/html; charset=utf-8".into(),
            }
        );
    }
}
