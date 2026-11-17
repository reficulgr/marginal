//! SQLite persistence (rusqlite). One library database + per-book sidecar files
//! for portability (see sidecar.rs).

use crate::model::*;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database at {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Store> {
        Store::open(&data_dir().join("glossa.db"))
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS books (
                id           INTEGER PRIMARY KEY,
                fingerprint  TEXT NOT NULL UNIQUE,
                path         TEXT NOT NULL UNIQUE,
                title        TEXT NOT NULL,
                author       TEXT,
                format       TEXT NOT NULL,
                page_count   INTEGER,
                size_bytes   INTEGER NOT NULL DEFAULT 0,
                mtime        INTEGER NOT NULL DEFAULT 0,
                thumb        BLOB,
                added_at     INTEGER NOT NULL DEFAULT 0,
                last_opened  INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS annotations (
                id         TEXT PRIMARY KEY,
                book_id    INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
                page       INTEGER NOT NULL,
                kind       TEXT NOT NULL,
                anchor     TEXT NOT NULL,
                body       TEXT NOT NULL DEFAULT '',
                color      TEXT NOT NULL DEFAULT 'yellow',
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_ann_book ON annotations(book_id, page);

            CREATE TABLE IF NOT EXISTS links (
                id      TEXT PRIMARY KEY,
                src_id  TEXT NOT NULL REFERENCES annotations(id) ON DELETE CASCADE,
                target  TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_links_src ON links(src_id);",
        )?;
        Ok(())
    }

    // -- meta ---------------------------------------------------------------

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
    }

    // -- books --------------------------------------------------------------

    fn row_to_book(r: &Row) -> rusqlite::Result<BookMeta> {
        let fmt: String = r.get("format")?;
        let format = match fmt.to_ascii_lowercase().as_str() {
            "pdf" => BookFormat::Pdf,
            "epub" => BookFormat::Epub,
            "mobi" => BookFormat::Mobi,
            "azw3" => BookFormat::Azw3,
            "fb2" => BookFormat::Fb2,
            "djvu" => BookFormat::Djvu,
            "cbz" => BookFormat::Cbz,
            _ => BookFormat::Other,
        };
        Ok(BookMeta {
            id: r.get("id")?,
            fingerprint: r.get("fingerprint")?,
            path: r.get("path")?,
            title: r.get("title")?,
            author: r.get("author")?,
            format,
            page_count: r.get("page_count")?,
            size_bytes: r.get("size_bytes")?,
            mtime: r.get("mtime")?,
            has_thumb: r.get::<_, Option<Vec<u8>>>("thumb")?.is_some(),
        })
    }

    pub fn book_by_id(&self, id: i64) -> Result<BookMeta> {
        Ok(self.conn.query_row(
            "SELECT * FROM books WHERE id=?1",
            params![id],
            Self::row_to_book,
        )?)
    }

    pub fn book_by_fingerprint(&self, fp: &str) -> Result<Option<BookMeta>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM books WHERE fingerprint=?1",
                params![fp],
                Self::row_to_book,
            )
            .optional()?)
    }

    pub fn books(&self) -> Result<Vec<BookMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM books ORDER BY title COLLATE NOCASE")?;
        let rows = stmt.query_map([], Self::row_to_book)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Returns true when the database already has this path with identical
    /// size + mtime (cheap re-scan fast-path; no hashing needed).
    pub fn book_unchanged(&self, path: &str, size: i64, mtime: i64) -> Result<bool> {
        let found: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT size_bytes, mtime FROM books WHERE path=?1",
                params![path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(matches!(found, Some((s, m)) if s == size && m == mtime))
    }

    /// Insert or correct book facts. `fingerprint` is source of truth.
    pub fn upsert_book(
        &self,
        fingerprint: &str,
        path: &str,
        title: &str,
        format: BookFormat,
        size: i64,
        mtime: i64,
    ) -> Result<i64> {
        let now = now_secs();
        // Existing row, same fingerprint (book moved or re-added)?
        let by_fp: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM books WHERE fingerprint=?1",
                params![fingerprint],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = by_fp {
            self.conn.execute(
                "UPDATE books SET path=?2, size_bytes=?3, mtime=?4 WHERE id=?1",
                params![id, path, size, mtime],
            )?;
            return Ok(id);
        }
        // Existing row at same path but different content (file replaced)?
        let by_path: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM books WHERE path=?1",
                params![path],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = by_path {
            self.conn.execute(
                "UPDATE books SET fingerprint=?2, size_bytes=?3, mtime=?4,
                                   page_count=NULL, thumb=NULL WHERE id=?1",
                params![id, fingerprint, size, mtime],
            )?;
            return Ok(id);
        }
        // New book.
        self.conn.execute(
            "INSERT INTO books(fingerprint, path, title, author, format, size_bytes, mtime, added_at)
             VALUES(?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)",
            params![fingerprint, path, title, format.label(), size, mtime, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_thumb(&self, id: i64, png: &[u8]) -> Result<()> {
        self.conn.execute(
            "UPDATE books SET thumb=?2 WHERE id=?1",
            params![id, png],
        )?;
        Ok(())
    }

    pub fn set_page_count(&self, id: i64, n: u32) -> Result<()> {
        self.conn
            .execute("UPDATE books SET page_count=?2 WHERE id=?1", params![id, n as i64])?;
        Ok(())
    }

    pub fn thumb(&self, id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT thumb FROM books WHERE id=?1", params![id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    pub fn touch_opened(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE books SET last_opened=?2 WHERE id=?1",
            params![id, now_secs()],
        )?;
        Ok(())
    }

    // -- annotations ---------------------------------------------------------

    fn row_to_annotation(r: &Row) -> rusqlite::Result<Annotation> {
        let anchor_raw: String = r.get("anchor")?;
        let anchor = serde_json::from_str(&anchor_raw)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e)))?;
        Ok(Annotation {
            id: r.get("id")?,
            page: r.get::<_, i64>("page")? as u32,
            anchor,
            body: r.get("body")?,
            color: r.get("color")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }

    pub fn annotations_for_book(&self, book_id: i64) -> Result<Vec<Annotation>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM annotations WHERE book_id=?1 ORDER BY page, created_at",
        )?;
        let rows = stmt.query_map(params![book_id], Self::row_to_annotation)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_annotation(&self, book_id: i64, ann: &Annotation) -> Result<()> {
        self.conn.execute(
            "INSERT INTO annotations(id, book_id, page, kind, anchor, body, color, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                ann.id,
                book_id,
                ann.page as i64,
                annotation_kind(&ann.anchor),
                serde_json::to_string(&ann.anchor)?,
                ann.body,
                ann.color,
                ann.created_at,
                ann.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn update_annotation(&self, id: &str, body: &str, color: &str, page: u32, anchor: &Anchor, updated_at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE annotations SET body=?2, color=?3, page=?4, anchor=?5, updated_at=?6 WHERE id=?1",
            params![id, body, color, page as i64, serde_json::to_string(anchor)?, updated_at],
        )?;
        Ok(())
    }

    pub fn delete_annotation(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM annotations WHERE id=?1", params![id])?;
        Ok(())
    }

    // -- links ---------------------------------------------------------------

    pub fn links_for(&self, src_id: &str) -> Result<Vec<(String, LinkTarget)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, target FROM links WHERE src_id=?1 ORDER BY created_at")?;
        let rows = stmt.query_map(params![src_id], |r| {
            let id: String = r.get(0)?;
            let raw: String = r.get(1)?;
            let target: LinkTarget =
                serde_json::from_str(&raw).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
            Ok((id, target))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_link(&self, src_id: &str, target: &LinkTarget) -> Result<String> {
        // Dedupe: identical (src, target) links.
        let existing = self.links_for(src_id)?;
        if existing.iter().any(|(_, t)| t == target) {
            return Ok(existing
                .iter()
                .find(|(_, t)| t == target)
                .unwrap()
                .0
                .clone());
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO links(id, src_id, target, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![id, src_id, serde_json::to_string(target)?, now_secs()],
        )?;
        Ok(id)
    }

    pub fn delete_link(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM links WHERE id=?1", params![id])?;
        Ok(())
    }

    /// All links originating from annotations of a book: (annotation_id, link_id, target).
    pub fn links_of_book(&self, book_id: i64) -> Result<Vec<(String, String, LinkTarget)>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.src_id, l.id, l.target FROM links l
             JOIN annotations a ON a.id = l.src_id
             WHERE a.book_id = ?1 ORDER BY l.created_at",
        )?;
        let rows = stmt.query_map(params![book_id], |r| {
            let src: String = r.get(0)?;
            let id: String = r.get(1)?;
            let raw: String = r.get(2)?;
            let target: LinkTarget =
                serde_json::from_str(&raw).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
            Ok((src, id, target))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn annotation_kind(a: &Anchor) -> &'static str {
    match a {
        Anchor::PostIt { .. } => "postit",
        Anchor::Margin { .. } => "margin",
    }
}

/// Internal helper: book + stored stat, used by scanning.
#[allow(dead_code)]
pub struct BookWithStat {
    #[allow(dead_code)]
    pub book: BookMeta,
}

/// Well-known data directory (XDG) for glossa's database and pdfium binary.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("glossa")
}

/// Shared handle so background scan/thumb workers can use the store too.
pub type SharedStore = Arc<Mutex<Store>>;

pub fn with_store<T>(store: &SharedStore, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
    let g = store
        .lock()
        .map_err(|_| anyhow!("store mutex poisoned"))?;
    f(&g)
}
