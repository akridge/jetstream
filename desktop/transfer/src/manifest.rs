//! Local record of what has been uploaded, so repeat syncs skip unchanged files without
//! listing the bucket, plus saved resumable sessions so large uploads continue after a restart.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq)]
pub struct FileRecord {
    pub size: u64,
    pub mtime_ns: i64,
    pub crc32c: u32,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub size: u64,
    pub mtime_ns: i64,
    pub crc32c: u32,
    pub session_uri: String,
}

pub struct Manifest {
    conn: Mutex<Connection>,
}

/// JetStream's per-user data folder (same rules as the Python app's `user_data_dir`).
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JETSTREAM_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("JetStream")
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support").join("JetStream")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("jetstream")
    }
}

impl Manifest {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("could not open manifest {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS files (
                 dest TEXT NOT NULL,
                 rel_path TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 crc32c INTEGER NOT NULL,
                 generation TEXT,
                 synced_at INTEGER NOT NULL,
                 PRIMARY KEY (dest, rel_path)
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 dest TEXT NOT NULL,
                 rel_path TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 crc32c INTEGER NOT NULL,
                 session_uri TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (dest, rel_path)
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn count(&self, dest: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT COUNT(*) FROM files WHERE dest = ?1", [dest], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Every recorded file for a destination, keyed by relative path.
    pub fn load(&self, dest: &str) -> Result<std::collections::HashMap<String, FileRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT rel_path, size, mtime_ns, crc32c FROM files WHERE dest = ?1")?;
        let rows = stmt.query_map([dest], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FileRecord { size: r.get::<_, i64>(1)? as u64, mtime_ns: r.get(2)?, crc32c: r.get::<_, i64>(3)? as u32 },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn record(&self, dest: &str, rel_path: &str, file: &FileRecord, generation: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO files (dest, rel_path, size, mtime_ns, crc32c, generation, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'))
             ON CONFLICT(dest, rel_path) DO UPDATE SET
                 size = excluded.size, mtime_ns = excluded.mtime_ns, crc32c = excluded.crc32c,
                 generation = COALESCE(excluded.generation, files.generation), synced_at = excluded.synced_at",
            params![dest, rel_path, file.size as i64, file.mtime_ns, file.crc32c as i64, generation],
        )?;
        Ok(())
    }

    pub fn session(&self, dest: &str, rel_path: &str) -> Result<Option<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT size, mtime_ns, crc32c, session_uri FROM sessions WHERE dest = ?1 AND rel_path = ?2",
                params![dest, rel_path],
                |r| {
                    Ok(SessionRecord {
                        size: r.get::<_, i64>(0)? as u64,
                        mtime_ns: r.get(1)?,
                        crc32c: r.get::<_, i64>(2)? as u32,
                        session_uri: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn save_session(&self, dest: &str, rel_path: &str, file: &FileRecord, session_uri: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO sessions (dest, rel_path, size, mtime_ns, crc32c, session_uri, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'))",
            params![dest, rel_path, file.size as i64, file.mtime_ns, file.crc32c as i64, session_uri],
        )?;
        Ok(())
    }

    pub fn clear_session(&self, dest: &str, rel_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE dest = ?1 AND rel_path = ?2", params![dest, rel_path])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_sessions_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::open(&dir.path().join("m.db")).unwrap();
        let file = FileRecord { size: 10, mtime_ns: 5, crc32c: 0xFFFF_FFFF };
        manifest.record("gs://b/p/", "a.jpg", &file, Some("1")).unwrap();
        manifest.record("gs://b/other/", "a.jpg", &file, None).unwrap();
        assert_eq!(manifest.count("gs://b/p/").unwrap(), 1);
        assert_eq!(manifest.load("gs://b/p/").unwrap()["a.jpg"], file);

        let newer = FileRecord { size: 11, ..file.clone() };
        manifest.record("gs://b/p/", "a.jpg", &newer, None).unwrap();
        assert_eq!(manifest.load("gs://b/p/").unwrap()["a.jpg"].size, 11);

        assert!(manifest.session("gs://b/p/", "big.bin").unwrap().is_none());
        manifest.save_session("gs://b/p/", "big.bin", &file, "https://upload/session").unwrap();
        assert_eq!(manifest.session("gs://b/p/", "big.bin").unwrap().unwrap().session_uri, "https://upload/session");
        manifest.clear_session("gs://b/p/", "big.bin").unwrap();
        assert!(manifest.session("gs://b/p/", "big.bin").unwrap().is_none());
    }
}
