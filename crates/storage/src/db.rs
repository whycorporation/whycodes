use crate::migrations::run_migrations;
use crate::models::{
    CodeChunkRow, MemoryRow, MessageRow, SessionChunkRow, SessionRow, UsageTotals,
};
use rusqlite::Connection;
use whycode_core::types::Usage;

/// Core database wrapper
pub struct Database {
    conn: Connection,
}

/// Message tuple accepted by [`Database::replace_messages`]:
/// (msg id, role, content json, tool_call_id, name, created_at rfc3339).
pub type MessageInsert = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

/// How long to wait for another process to release a database lock before
/// giving up. Without this SQLite returns SQLITE_BUSY immediately, so two
/// whycode processes touching the same database can fail spuriously.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Database {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;

        // Switching journal mode needs exclusive access, so it returns
        // SQLITE_BUSY whenever another connection holds a transaction — and
        // when that is a write transaction it returns immediately, without
        // consulting the busy timeout set above. The mode only has to change
        // once: it lives in the file header, so a failure here means either
        // that another process already set WAL or that this connection runs in
        // the default rollback mode. Neither is fatal, so do not propagate it.
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    /// Insert or update a session row, preserving `created_at` on conflict.
    pub fn upsert_session(
        &self,
        id: &str,
        title: &str,
        project_path: &str,
        created_at: &str,
        updated_at: &str,
        usage: &Usage,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (
                id, title, created_at, updated_at, project_path,
                input_tokens, output_tokens,
                cache_creation_input_tokens, cache_read_input_tokens
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                updated_at = excluded.updated_at,
                project_path = excluded.project_path,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_creation_input_tokens = excluded.cache_creation_input_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens",
            rusqlite::params![
                id,
                title,
                created_at,
                updated_at,
                project_path,
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                usage.cache_creation_input_tokens.map(|v| v as i64),
                usage.cache_read_input_tokens.map(|v| v as i64),
            ],
        )?;
        Ok(())
    }

    /// Convenience for tests / simple creates (zero usage, now timestamps).
    pub fn create_session(&self, id: &str, title: &str, project_path: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.upsert_session(id, title, project_path, &now, &now, &Usage::default())
    }

    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, project_path,
                    input_tokens, output_tokens,
                    cache_creation_input_tokens, cache_read_input_tokens
             FROM sessions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], map_session_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at, project_path,
                    input_tokens, output_tokens,
                    cache_creation_input_tokens, cache_read_input_tokens
             FROM sessions ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], map_session_row)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Sum provider-reported usage and message counts across all sessions.
    pub fn usage_totals(&self) -> anyhow::Result<UsageTotals> {
        let (session_count, input, output, cache_create, cache_read): (
            i64,
            i64,
            i64,
            Option<i64>,
            Option<i64>,
        ) = self.conn.query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                SUM(cache_creation_input_tokens),
                SUM(cache_read_input_tokens)
             FROM sessions",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;

        let message_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;

        // SUM of all-NULL cache columns is NULL; keep Option semantics.
        let usage = Usage {
            input_tokens: input as u64,
            output_tokens: output as u64,
            cache_creation_input_tokens: cache_create.map(|v| v as u64),
            cache_read_input_tokens: cache_read.map(|v| v as u64),
        };

        Ok(UsageTotals {
            session_count: session_count as usize,
            message_count: message_count as usize,
            usage,
        })
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
        self.conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            rusqlite::params![id],
        )?;
        self.conn
            .execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn insert_message(
        &self,
        msg_id: &str,
        session_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        name: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO messages (id, session_id, role, content, tool_call_id, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![msg_id, session_id, role, content, tool_call_id, name, now],
        )?;
        Ok(())
    }

    /// Drop all messages for a session (used before full re-persist).
    pub fn delete_messages(&self, session_id: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// Replace every message for a session in one transaction.
    ///
    /// Full rewrite keeps the table honest across multi-turn persists; the
    /// transaction avoids half-written transcripts if a process dies mid-loop.
    pub fn replace_messages(
        &self,
        session_id: &str,
        messages: &[MessageInsert],
    ) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        for (msg_id, role, content, tool_call_id, name, created_at) in messages {
            tx.execute(
                "INSERT INTO messages (id, session_id, role, content, tool_call_id, name, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    msg_id,
                    session_id,
                    role,
                    content,
                    tool_call_id,
                    name,
                    created_at
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Message counts for every session in one query (session picker).
    pub fn message_counts_by_session(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, usize>> {
        let mut stmt = self
            .conn
            .prepare("SELECT session_id, COUNT(*) FROM messages GROUP BY session_id")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let n: i64 = row.get(1)?;
            Ok((id, n as usize))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, n) = row?;
            map.insert(id, n);
        }
        Ok(map)
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
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM state WHERE key = ?1")?;
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
        self.conn
            .execute("DELETE FROM state WHERE key = ?1", rusqlite::params![key])?;
        Ok(())
    }

    // ── Memories (semantic / auto memory) ────────────────────────────────

    pub fn insert_memory(
        &self,
        id: &str,
        project_key: &str,
        text: &str,
        embedding: &[u8],
        source_session: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO memories (
                id, project_key, text, embedding, source_session,
                created_at, last_recalled_at, recall_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 0)",
            rusqlite::params![id, project_key, text, embedding, source_session, now],
        )?;
        Ok(())
    }

    pub fn get_memory(&self, id: &str) -> anyhow::Result<Option<MemoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_key, text, embedding, source_session,
                    created_at, last_recalled_at, recall_count
             FROM memories WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], map_memory_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_memories(&self, project_key: &str, limit: usize) -> anyhow::Result<Vec<MemoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_key, text, embedding, source_session,
                    created_at, last_recalled_at, recall_count
             FROM memories WHERE project_key = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_key, limit as i64], map_memory_row)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn delete_memory(&self, id: &str) -> anyhow::Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])?;
        Ok(n > 0)
    }

    pub fn clear_memories(&self, project_key: &str) -> anyhow::Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM memories WHERE project_key = ?1",
            rusqlite::params![project_key],
        )?;
        Ok(n)
    }

    pub fn touch_memory_recall(&self, id: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE memories SET last_recalled_at = ?1, recall_count = recall_count + 1
             WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    pub fn count_memories(&self, project_key: &str) -> anyhow::Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE project_key = ?1",
            rusqlite::params![project_key],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    // ── Code chunks (codebase RAG) ───────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn insert_code_chunk(
        &self,
        id: &str,
        project_key: &str,
        path: &str,
        start_line: i64,
        end_line: i64,
        text: &str,
        embedding: &[u8],
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO code_chunks (
                id, project_key, path, start_line, end_line, text, embedding, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                id,
                project_key,
                path,
                start_line,
                end_line,
                text,
                embedding,
                now
            ],
        )?;
        Ok(())
    }

    pub fn list_code_chunks(
        &self,
        project_key: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<CodeChunkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_key, path, start_line, end_line, text, embedding, updated_at
             FROM code_chunks WHERE project_key = ?1
             ORDER BY path ASC, start_line ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![project_key, limit as i64],
            map_code_chunk_row,
        )?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn clear_code_chunks(&self, project_key: &str) -> anyhow::Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM code_chunks WHERE project_key = ?1",
            rusqlite::params![project_key],
        )?;
        Ok(n)
    }

    // ── Session chunks (turn RAG) ────────────────────────────────────────

    pub fn insert_session_chunk(
        &self,
        id: &str,
        project_key: &str,
        session_id: &str,
        turn_index: i64,
        text: &str,
        embedding: &[u8],
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO session_chunks (
                id, project_key, session_id, turn_index, text, embedding, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                project_key,
                session_id,
                turn_index,
                text,
                embedding,
                now
            ],
        )?;
        Ok(())
    }

    pub fn list_session_chunks(
        &self,
        project_key: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SessionChunkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_key, session_id, turn_index, text, embedding, created_at
             FROM session_chunks WHERE project_key = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![project_key, limit as i64],
            map_session_chunk_row,
        )?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

fn map_memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
    Ok(MemoryRow {
        id: row.get(0)?,
        project_key: row.get(1)?,
        text: row.get(2)?,
        embedding: row.get(3)?,
        source_session: row.get(4)?,
        created_at: row.get(5)?,
        last_recalled_at: row.get(6)?,
        recall_count: row.get(7)?,
    })
}

fn map_session_chunk_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionChunkRow> {
    Ok(SessionChunkRow {
        id: row.get(0)?,
        project_key: row.get(1)?,
        session_id: row.get(2)?,
        turn_index: row.get(3)?,
        text: row.get(4)?,
        embedding: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn map_code_chunk_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeChunkRow> {
    Ok(CodeChunkRow {
        id: row.get(0)?,
        project_key: row.get(1)?,
        path: row.get(2)?,
        start_line: row.get(3)?,
        end_line: row.get(4)?,
        text: row.get(5)?,
        embedding: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    let input: i64 = row.get(5)?;
    let output: i64 = row.get(6)?;
    let cache_create: Option<i64> = row.get(7)?;
    let cache_read: Option<i64> = row.get(8)?;
    Ok(SessionRow {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        project_path: row.get(4)?,
        usage: Usage {
            input_tokens: input as u64,
            output_tokens: output as u64,
            cache_creation_input_tokens: cache_create.map(|v| v as u64),
            cache_read_input_tokens: cache_read.map(|v| v as u64),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    /// Opening a database while another connection holds a write transaction
    /// must succeed. `PRAGMA journal_mode=WAL` returns SQLITE_BUSY in that
    /// state without consulting the busy handler, and treating that as fatal
    /// made concurrent whycode processes fail with "database is locked".
    #[test]
    fn test_open_while_another_connection_holds_a_write_transaction() {
        let path = std::env::temp_dir().join(format!("whycode-test-{}.db", uuid::Uuid::new_v4()));
        let path_str = path.to_string_lossy().to_string();

        // Seed the schema with a plain connection, leaving the journal mode at
        // the default. `Database::open` therefore has to switch it to WAL,
        // which is the step that fails under contention — on a database that
        // is already in WAL mode the pragma is a no-op and takes no lock.
        let seed = Connection::open(&path_str).unwrap();
        run_migrations(&seed).unwrap();
        seed.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let opened = Database::open(&path_str);

        let _ = seed.execute_batch("ROLLBACK;");
        drop(seed);
        for suffix in ["db", "db-wal", "db-shm"] {
            let _ = std::fs::remove_file(path.with_extension(suffix));
        }

        opened.expect("open failed while another connection held a write transaction");
    }

    #[test]
    fn test_create_and_get_session() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        let s = db.get_session("s1").unwrap().unwrap();
        assert_eq!(s.title, "Test");
        assert!(s.usage.is_empty());
    }

    #[test]
    fn test_upsert_preserves_created_at_and_stores_usage() {
        let db = test_db();
        let created = "2020-01-01T00:00:00+00:00";
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: Some(1),
            cache_read_input_tokens: Some(2),
        };
        db.upsert_session("s1", "Old", "/tmp", created, created, &usage)
            .unwrap();

        let updated = "2021-06-15T12:00:00+00:00";
        let usage2 = Usage {
            input_tokens: 100,
            output_tokens: 200,
            cache_creation_input_tokens: Some(3),
            cache_read_input_tokens: Some(4),
        };
        db.upsert_session("s1", "New", "/proj", created, updated, &usage2)
            .unwrap();

        let s = db.get_session("s1").unwrap().unwrap();
        assert_eq!(s.title, "New");
        assert_eq!(s.created_at, created);
        assert_eq!(s.updated_at, updated);
        assert_eq!(s.project_path, "/proj");
        assert_eq!(s.usage.input_tokens, 100);
        assert_eq!(s.usage.output_tokens, 200);
        assert_eq!(s.usage.cache_creation_input_tokens, Some(3));
        assert_eq!(s.usage.cache_read_input_tokens, Some(4));
    }

    #[test]
    fn test_usage_totals() {
        let db = test_db();
        let now = chrono::Utc::now().to_rfc3339();
        db.upsert_session(
            "a",
            "A",
            "/a",
            &now,
            &now,
            &Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: Some(1),
                cache_read_input_tokens: None,
            },
        )
        .unwrap();
        db.upsert_session(
            "b",
            "B",
            "/b",
            &now,
            &now,
            &Usage {
                input_tokens: 20,
                output_tokens: 7,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: Some(4),
            },
        )
        .unwrap();
        db.insert_message("m1", "a", "user", "hi", None, None)
            .unwrap();
        db.insert_message("m2", "a", "assistant", "yo", None, None)
            .unwrap();

        let t = db.usage_totals().unwrap();
        assert_eq!(t.session_count, 2);
        assert_eq!(t.message_count, 2);
        assert_eq!(t.usage.input_tokens, 30);
        assert_eq!(t.usage.output_tokens, 12);
        assert_eq!(t.usage.cache_creation_input_tokens, Some(1));
        assert_eq!(t.usage.cache_read_input_tokens, Some(4));
        assert_eq!(t.usage.total(), 30 + 12 + 1 + 4);
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
        db.insert_message("m1", "s1", "user", "hi", None, None)
            .unwrap();
        db.delete_session("s1").unwrap();
        assert!(db.get_session("s1").unwrap().is_none());
        assert!(db.get_messages("s1").unwrap().is_empty());
    }

    #[test]
    fn test_insert_and_get_messages() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        db.insert_message("m1", "s1", "user", "hello", None, None)
            .unwrap();
        db.insert_message("m2", "s1", "assistant", "hi", None, None)
            .unwrap();
        let msgs = db.get_messages("s1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn test_delete_messages() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        db.insert_message("m1", "s1", "user", "hi", None, None)
            .unwrap();
        db.delete_messages("s1").unwrap();
        assert_eq!(db.message_count("s1").unwrap(), 0);
    }

    #[test]
    fn test_message_count() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        assert_eq!(db.message_count("s1").unwrap(), 0);
        db.insert_message("m1", "s1", "user", "hi", None, None)
            .unwrap();
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

    #[test]
    fn test_open_file_roundtrip_and_bad_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("whycode.db");
        let path_str = path.to_str().unwrap();
        let db = Database::open(path_str).unwrap();
        db.create_session("s1", "File", "/proj").unwrap();
        drop(db);

        let db = Database::open(path_str).unwrap();
        assert_eq!(db.get_session("s1").unwrap().unwrap().title, "File");

        assert!(Database::open(tmp.path().to_str().unwrap()).is_err());
        assert!(Database::open("/no/such/whycode-storage-dir/db.sqlite").is_err());
    }

    #[test]
    fn test_replace_messages_and_counts() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        db.insert_message("old", "s1", "user", "stale", None, None)
            .unwrap();
        db.insert_message("tool1", "s1", "tool", "out", Some("call-1"), Some("bash"))
            .unwrap();

        let created = "2020-01-01T00:00:00+00:00".to_string();
        db.replace_messages(
            "s1",
            &[
                (
                    "m1".into(),
                    "user".into(),
                    "hello".into(),
                    None,
                    None,
                    created.clone(),
                ),
                (
                    "m2".into(),
                    "tool".into(),
                    "done".into(),
                    Some("tc1".into()),
                    Some("grep".into()),
                    created,
                ),
            ],
        )
        .unwrap();

        let msgs = db.get_messages("s1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, "m1");
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("tc1"));
        assert_eq!(msgs[1].name.as_deref(), Some("grep"));

        let counts = db.message_counts_by_session().unwrap();
        assert_eq!(counts.get("s1"), Some(&2));

        db.replace_messages("s1", &[]).unwrap();
        assert!(db.get_messages("s1").unwrap().is_empty());
        assert!(db.message_counts_by_session().unwrap().is_empty());
    }

    #[test]
    fn test_replace_messages_duplicate_id_fails() {
        let db = test_db();
        db.create_session("s1", "Test", "/tmp").unwrap();
        let created = "2020-01-01T00:00:00+00:00".to_string();
        let err = db.replace_messages(
            "s1",
            &[
                (
                    "m1".into(),
                    "user".into(),
                    "a".into(),
                    None,
                    None,
                    created.clone(),
                ),
                ("m1".into(), "user".into(), "b".into(), None, None, created),
            ],
        );
        assert!(err.is_err());
    }

    #[test]
    fn test_insert_message_foreign_key() {
        let db = test_db();
        assert!(
            db.insert_message("m1", "missing", "user", "hi", None, None)
                .is_err()
        );
    }

    #[test]
    fn test_usage_totals_empty_database() {
        let db = test_db();
        let t = db.usage_totals().unwrap();
        assert_eq!(t.session_count, 0);
        assert_eq!(t.message_count, 0);
        assert!(t.usage.is_empty());
    }

    #[test]
    fn test_memories_crud() {
        let db = test_db();
        let emb = [1_u8, 2, 3, 4];
        db.insert_memory("mem1", "proj", "note", &emb, Some("s1"))
            .unwrap();
        db.insert_memory("mem2", "proj", "other", &emb, None)
            .unwrap();
        db.insert_memory("mem3", "elsewhere", "x", &emb, None)
            .unwrap();

        let got = db.get_memory("mem1").unwrap().unwrap();
        assert_eq!(got.project_key, "proj");
        assert_eq!(got.text, "note");
        assert_eq!(got.embedding, emb);
        assert_eq!(got.source_session.as_deref(), Some("s1"));
        assert!(got.last_recalled_at.is_none());
        assert_eq!(got.recall_count, 0);
        assert!(db.get_memory("missing").unwrap().is_none());

        let listed = db.list_memories("proj", 1).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(db.count_memories("proj").unwrap(), 2);

        db.touch_memory_recall("mem1").unwrap();
        let got = db.get_memory("mem1").unwrap().unwrap();
        assert_eq!(got.recall_count, 1);
        assert!(got.last_recalled_at.is_some());

        assert!(db.delete_memory("mem2").unwrap());
        assert!(!db.delete_memory("mem2").unwrap());
        assert_eq!(db.count_memories("proj").unwrap(), 1);
        assert_eq!(db.clear_memories("proj").unwrap(), 1);
        assert_eq!(db.count_memories("proj").unwrap(), 0);
        assert!(db.list_memories("proj", 10).unwrap().is_empty());
    }

    #[test]
    fn test_code_chunks() {
        let db = test_db();
        let emb = [9_u8, 8, 7];
        db.insert_code_chunk("c1", "proj", "src/a.rs", 1, 10, "fn a() {}", &emb)
            .unwrap();
        db.insert_code_chunk("c2", "proj", "src/b.rs", 2, 4, "fn b() {}", &emb)
            .unwrap();
        db.insert_code_chunk("c3", "other", "x.rs", 1, 1, "x", &emb)
            .unwrap();

        let rows = db.list_code_chunks("proj", 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, "src/a.rs");
        assert_eq!(rows[0].start_line, 1);
        assert_eq!(rows[0].end_line, 10);
        assert_eq!(rows[0].text, "fn a() {}");
        assert_eq!(rows[0].embedding, emb);
        assert_eq!(db.list_code_chunks("proj", 1).unwrap().len(), 1);
        assert_eq!(db.clear_code_chunks("proj").unwrap(), 2);
        assert!(db.list_code_chunks("proj", 10).unwrap().is_empty());
    }

    #[test]
    fn test_session_chunks() {
        let db = test_db();
        let emb = [4_u8, 5];
        db.insert_session_chunk("sc1", "proj", "s1", 0, "turn0", &emb)
            .unwrap();
        db.insert_session_chunk("sc2", "proj", "s1", 1, "turn1", &emb)
            .unwrap();
        db.insert_session_chunk("sc3", "other", "s2", 0, "x", &emb)
            .unwrap();

        let rows = db.list_session_chunks("proj", 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.project_key == "proj"));
        assert!(rows.iter().any(|r| r.turn_index == 0 && r.text == "turn0"));
        assert_eq!(rows.iter().find(|r| r.id == "sc1").unwrap().embedding, emb);
        assert_eq!(db.list_session_chunks("proj", 1).unwrap().len(), 1);
    }

    #[test]
    fn test_list_chunk_query_map_errors_when_database_is_locked() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("locked.db");
        let path_str = path.to_str().unwrap();
        let db = Database::open(path_str).unwrap();
        let emb = [1_u8];
        db.insert_code_chunk("c1", "proj", "a.rs", 1, 2, "t", &emb)
            .unwrap();
        db.insert_session_chunk("sc1", "proj", "s1", 0, "t", &emb)
            .unwrap();

        // WAL readers do not block; roll back to a journal mode where an
        // exclusive lock makes SELECT return SQLITE_BUSY immediately.
        db.conn
            .execute_batch("PRAGMA journal_mode=DELETE;")
            .unwrap();
        db.conn
            .busy_timeout(std::time::Duration::from_millis(0))
            .unwrap();

        let blocker = Connection::open(path_str).unwrap();
        blocker
            .busy_timeout(std::time::Duration::from_millis(0))
            .unwrap();
        blocker.execute_batch("BEGIN EXCLUSIVE;").unwrap();

        assert!(db.list_code_chunks("proj", 10).is_err());
        assert!(db.list_session_chunks("proj", 10).is_err());

        blocker.execute_batch("ROLLBACK;").unwrap();
    }

    #[test]
    fn test_sql_error_paths_after_dropping_schema() {
        let db = test_db();
        db.conn
            .execute_batch(
                "
                DROP TABLE IF EXISTS session_chunks;
                DROP TABLE IF EXISTS code_chunks;
                DROP TABLE IF EXISTS memories;
                DROP TABLE IF EXISTS messages;
                DROP TABLE IF EXISTS sessions;
                DROP TABLE IF EXISTS state;
                ",
            )
            .unwrap();

        let usage = Usage::default();
        let now = "2020-01-01T00:00:00+00:00";
        assert!(db.upsert_session("s", "t", "/p", now, now, &usage).is_err());
        assert!(db.create_session("s", "t", "/p").is_err());
        assert!(db.get_session("s").is_err());
        assert!(db.list_sessions().is_err());
        assert!(db.usage_totals().is_err());
        assert!(db.update_title("s", "t").is_err());
        assert!(db.delete_session("s").is_err());
        assert!(
            db.insert_message("m", "s", "user", "hi", None, None)
                .is_err()
        );
        assert!(db.delete_messages("s").is_err());
        assert!(db.replace_messages("s", &[]).is_err());
        assert!(db.message_counts_by_session().is_err());
        assert!(db.get_messages("s").is_err());
        assert!(db.message_count("s").is_err());
        assert!(db.get_state("k").is_err());
        assert!(db.set_state("k", "v").is_err());
        assert!(db.delete_state("k").is_err());
        assert!(db.insert_memory("id", "pk", "t", &[], None).is_err());
        assert!(db.get_memory("id").is_err());
        assert!(db.list_memories("pk", 10).is_err());
        assert!(db.delete_memory("id").is_err());
        assert!(db.clear_memories("pk").is_err());
        assert!(db.touch_memory_recall("id").is_err());
        assert!(db.count_memories("pk").is_err());
        assert!(
            db.insert_code_chunk("id", "pk", "p.rs", 1, 2, "t", &[])
                .is_err()
        );
        assert!(db.list_code_chunks("pk", 10).is_err());
        assert!(db.clear_code_chunks("pk").is_err());
        assert!(
            db.insert_session_chunk("id", "pk", "s", 0, "t", &[])
                .is_err()
        );
        assert!(db.list_session_chunks("pk", 10).is_err());
    }
}
