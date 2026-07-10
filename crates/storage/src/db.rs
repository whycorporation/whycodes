use rusqlite::Connection;
use crate::migrations::run_migrations;
use crate::models::{SessionRow, MessageRow, StateRow};

/// Core database wrapper
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn create_session(&self, id: &str, title: &str, project_path: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, title, created_at, updated_at, project_path) VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, title, now, project_path],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, project_path FROM sessions WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                project_path: row.get(4)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, project_path FROM sessions ORDER BY updated_at DESC"
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
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn update_title(&self, id: &str, title: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![title, now, id],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn insert_message(&self, msg_id: &str, session_id: &str, role: &str, content: &str, tool_call_id: Option<&str>, name: Option<&str>) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO messages (id, session_id, role, content, tool_call_id, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![msg_id, session_id, role, content, tool_call_id, name, now],
        )?;
        Ok(())
    }

    pub fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, tool_call_id, name, created_at FROM messages WHERE session_id = ?1 ORDER BY created_at ASC"
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
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
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn message_count(&self, session_id: &str) -> anyhow::Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn get_state(&self, key: &str) -> anyhow::Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM state WHERE key = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![key], |row| row.get::<_, String>(0))?;
        Ok(rows.next().transpose()?)
    }

    pub fn set_state(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_state(&self, key: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM state WHERE key = ?1", rusqlite::params![key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn test_create_and_get_session() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        let s = db.get_session("s1").unwrap().unwrap();
        assert_eq!(s.title, "Test");
    }

    #[test]
    fn test_list_sessions() {
        let db = test_db();
        db.create_session("a", "A", "/a").unwrap();
        db.create_session("b", "B", "/b").unwrap();
        assert_eq!(db.list_sessions().unwrap().len(), 2);
    }

    #[test]
    fn test_update_title() {
        let db = test_db();
        db.create_session("s1", "Old", "/tmp").unwrap();
        db.update_title("s1", "New").unwrap();
        assert_eq!(db.get_session("s1").unwrap().unwrap().title, "New");
    }

    #[test]
    fn test_delete_session() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        db.delete_session("s1").unwrap();
        assert!(db.get_session("s1").unwrap().is_none());
    }

    #[test]
    fn test_insert_and_get_messages() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        db.insert_message("m1", "s1", "user", "hello", None, None).unwrap();
        db.insert_message("m2", "s1", "assistant", "hi", None, None).unwrap();
        let msgs = db.get_messages("s1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn test_message_count() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        assert_eq!(db.message_count("s1").unwrap(), 0);
        db.insert_message("m1", "s1", "user", "hi", None, None).unwrap();
        assert_eq!(db.message_count("s1").unwrap(), 1);
    }

    #[test]
    fn test_state_get_set_delete() {
        let db = test_db();
        assert!(db.get_state("k1").unwrap().is_none());
        db.set_state("k1", "v1").unwrap();
        assert_eq!(db.get_state("k1").unwrap().unwrap(), "v1");
        db.delete_state("k1").unwrap();
        assert!(db.get_state("k1").unwrap().is_none());
    }

    #[test]
    fn test_nonexistent_session() {
        let db = test_db();
        assert!(db.get_session("no").unwrap().is_none());
    }
}
