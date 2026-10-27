//! SQLite: антидубли (по пользователю) + ожидание ответа.
use crate::models::Thread;
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS seen_items (
                key TEXT PRIMARY KEY,
                url TEXT,
                thread_id TEXT,
                title TEXT,
                section TEXT,
                author TEXT,
                first_seen TEXT DEFAULT CURRENT_TIMESTAMP,
                notified INTEGER DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS pending_reply (
                user_id TEXT PRIMARY KEY,
                thread_id TEXT,
                thread_url TEXT,
                text TEXT,
                created TEXT DEFAULT (strftime('%s','now'))
            );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ---- антидубли ----
    pub fn is_seen(&self, key: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT 1 FROM seen_items WHERE key = ?1", params![key], |_| Ok(()))
            .is_ok()
    }

    pub fn add_key(&self, key: &str, t: &Thread) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute(
            "INSERT OR IGNORE INTO seen_items (key, url, thread_id, title, section, author, notified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![key, t.url, t.id, t.title, t.section, t.author],
        );
    }

    pub fn mark_notified_key(&self, key: &str) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("UPDATE seen_items SET notified = 1 WHERE key = ?1", params![key]);
    }

    pub fn get_thread(&self, thread_id: &str) -> Option<(String, String, String)> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT url, title, section FROM seen_items
             WHERE thread_id = ?1 OR url LIKE ?2 LIMIT 1",
            params![thread_id, format!("%/threads/{thread_id}%")],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    }

    /// Есть ли уже темы этого раздела в базе (для «тихого» наполнения новых разделов).
    pub fn has_section(&self, section: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT 1 FROM seen_items WHERE section = ?1 LIMIT 1",
            params![section],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn count(&self) -> i64 {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT COUNT(*) FROM seen_items", [], |r| r.get(0)).unwrap_or(0)
    }

    // ---- ожидание ответа ----
    pub fn set_pending(&self, user_id: i64, thread_id: &str, url: &str) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute(
            "INSERT OR REPLACE INTO pending_reply (user_id, thread_id, thread_url, text, created)
             VALUES (?1, ?2, ?3, COALESCE((SELECT text FROM pending_reply WHERE user_id = ?1), ''), strftime('%s','now'))",
            params![user_id.to_string(), thread_id, url],
        );
    }

    pub fn set_pending_text(&self, user_id: i64, text: &str) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute(
            "UPDATE pending_reply SET text = ?1 WHERE user_id = ?2",
            params![text, user_id.to_string()],
        );
    }

    pub fn clear_pending_text(&self, user_id: i64) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute(
            "UPDATE pending_reply SET text = '' WHERE user_id = ?1",
            params![user_id.to_string()],
        );
    }

    pub fn get_pending(&self, user_id: i64) -> Option<(String, String)> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT thread_id, thread_url FROM pending_reply WHERE user_id = ?1",
            params![user_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    }

    pub fn clear_pending(&self, user_id: i64) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("DELETE FROM pending_reply WHERE user_id = ?1", params![user_id.to_string()]);
    }
}

/// Заглушка для совместимости (не используется).
pub fn has_pending_text_placeholder() -> bool {
    false
}
