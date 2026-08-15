//! Tags, colored labels, and comments per file, stored in a local SQLite
//! sidecar database (never touches the files themselves).

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct FileMeta {
    /// Index into `theme::LABEL_COLORS` (0 = none).
    pub label: u8,
    pub tags: String,
    pub comment: String,
}

pub struct TagStore {
    conn: Option<Connection>,
    /// Per-directory cache: dir -> (file name -> meta).
    cache: HashMap<PathBuf, HashMap<String, FileMeta>>,
}

impl TagStore {
    pub fn open(path: &Path) -> Self {
        let conn = Connection::open(path).ok();
        if let Some(c) = &conn {
            let _ = c.execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (
                    path TEXT PRIMARY KEY,
                    label INTEGER NOT NULL DEFAULT 0,
                    tags TEXT NOT NULL DEFAULT '',
                    comment TEXT NOT NULL DEFAULT ''
                );
                CREATE INDEX IF NOT EXISTS meta_dir ON meta (path);
                PRAGMA journal_mode=WAL;",
            );
        }
        Self { conn, cache: HashMap::new() }
    }

    /// Metadata for all files in a directory (cached).
    pub fn dir_meta(&mut self, dir: &Path) -> &HashMap<String, FileMeta> {
        if !self.cache.contains_key(dir) {
            let mut map = HashMap::new();
            if let Some(conn) = &self.conn {
                let prefix = format!("{}\\", dir.display());
                let like = format!("{}%", prefix.replace('%', "\\%"));
                if let Ok(mut stmt) = conn
                    .prepare("SELECT path, label, tags, comment FROM meta WHERE path LIKE ?1")
                {
                    let rows = stmt.query_map([&like], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    });
                    if let Ok(rows) = rows {
                        for row in rows.flatten() {
                            let (path, label, tags, comment) = row;
                            // Only direct children of `dir`.
                            if let Some(name) = path.strip_prefix(&prefix) {
                                if !name.contains('\\') {
                                    map.insert(
                                        name.to_string(),
                                        FileMeta {
                                            label: label as u8,
                                            tags,
                                            comment,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
            self.cache.insert(dir.to_path_buf(), map);
        }
        self.cache.get(dir).unwrap()
    }

    pub fn set_label(&mut self, path: &Path, label: u8) {
        self.upsert(path, |m| m.label = label);
    }

    pub fn set_tags(&mut self, path: &Path, tags: String) {
        self.upsert(path, |m| m.tags = tags);
    }

    pub fn set_comment(&mut self, path: &Path, comment: String) {
        self.upsert(path, |m| m.comment = comment);
    }

    pub fn get(&mut self, path: &Path) -> FileMeta {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return FileMeta::default();
        };
        let name = name.to_string_lossy().into_owned();
        self.dir_meta(dir).get(&name).cloned().unwrap_or_default()
    }

    fn upsert(&mut self, path: &Path, f: impl FnOnce(&mut FileMeta)) {
        let mut meta = self.get(path);
        f(&mut meta);
        if let Some(conn) = &self.conn {
            let _ = conn.execute(
                "INSERT INTO meta (path, label, tags, comment) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET label=?2, tags=?3, comment=?4",
                rusqlite::params![
                    path.display().to_string(),
                    meta.label as i64,
                    meta.tags,
                    meta.comment
                ],
            );
        }
        if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
            if let Some(map) = self.cache.get_mut(dir) {
                map.insert(name.to_string_lossy().into_owned(), meta);
            }
        }
    }

    /// Invalidate the cache for a directory (after external changes).
    pub fn invalidate(&mut self, dir: &Path) {
        self.cache.remove(dir);
    }
}
