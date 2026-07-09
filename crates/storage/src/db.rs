use rusqlite::{params, Connection};
use tracing::info;

use crate::migrations::run_migrations;
use crate::models::{MessageRow, SessionRow, StateRow};

/// SQLite-backed persistence store for sessions, messages, and key-value state.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the SQLite database at `path` and run migrations.
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for better concurrent-read performance.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        run_migrations(&conn)?;
        info!("Opened storage database at {}", path);

        Ok(Self { conn })
    }

    // ── session methods ────────────────────────────────────────

    pub fn create_session(
        &self,
        id: &str,
        title: &str,
        project_path: &str,
    ) -> Result<SessionRow, rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, project_path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, title, &now, &now, project_path],
        )?;

        Ok(SessionRow {
            id: id.to_string(),
            title: title.to_string(),
            created_at: now.clone(),
            updated_at: now,
            project_path: project_path.to_string(),
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, project_path
             FROM sessions WHERE id = ?1",
        )?;

        let mut rows = stmt.query_map(params![id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                project_path: row.get(4)?,
            })
        })?;

        match rows.next() {
            Some(row) => row.map(Some),
            None => Ok(None),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, project_path
             FROM sessions ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                project_path: row.get(4)?,
            })
        })?;

        rows.collect()
    }

    pub fn update_title(&self, id: &str, title: &str) -> Result<(), rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, &now, id],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<(), rusqlite::Error> {
        // Delete messages first (foreign key CASCADE would be cleaner,
        // but explicit is fine for portability).
        self.conn
            .execute("DELETE FROM messages WHERE session_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ── message methods ────────────────────────────────────────

    pub fn insert_message(
        &self,
        id: &str,
        session_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        name: Option<&str>,
    ) -> Result<MessageRow, rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO messages (id, session_id, role, content, tool_call_id, name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, session_id, role, content, tool_call_id, name, &now],
        )?;

        Ok(MessageRow {
            id: id.to_string(),
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            tool_call_id: tool_call_id.map(String::from),
            name: name.map(String::from),
            created_at: now,
        })
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<MessageRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, tool_call_id, name, created_at
             FROM messages WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                tool_call_id: row.get(4)?,
                name: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        rows.collect()
    }

    pub fn message_count(&self, session_id: &str) -> Result<usize, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, usize>(0),
        )
    }

    // ── state methods (key-value store) ────────────────────────

    pub fn get_state(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM state WHERE key = ?1")?;

        let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;

        match rows.next() {
            Some(row) => row.map(Some),
            None => Ok(None),
        }
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<StateRow, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;

        Ok(StateRow {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    pub fn delete_state(&self, key: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM state WHERE key = ?1", params![key])?;
        Ok(())
    }
}
