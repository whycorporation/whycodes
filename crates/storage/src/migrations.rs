use rusqlite::Connection;

/// Run all migrations on the given connection, creating tables
/// if they do not already exist and applying additive column upgrades.
pub fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            id          TEXT PRIMARY KEY,
            title       TEXT,
            created_at  TEXT,
            updated_at  TEXT,
            project_path TEXT
        );

        CREATE TABLE IF NOT EXISTS messages (
            id           TEXT PRIMARY KEY,
            session_id   TEXT,
            role         TEXT,
            content      TEXT,
            tool_call_id TEXT,
            name         TEXT,
            created_at   TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions(id)
        );
        -- Speeds delete/list/count by session (full replace on every persist).
        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);

        CREATE TABLE IF NOT EXISTS state (
            key   TEXT PRIMARY KEY,
            value TEXT
        );
        ",
    )?;

    // Additive: token usage on sessions (existing DBs pre-date these columns).
    ensure_column(
        conn,
        "sessions",
        "input_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        conn,
        "sessions",
        "output_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(conn, "sessions", "cache_creation_input_tokens", "INTEGER")?;
    ensure_column(conn, "sessions", "cache_read_input_tokens", "INTEGER")?;

    // Cross-session semantic / auto memory (plan-memory).
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memories (
            id               TEXT PRIMARY KEY,
            project_key      TEXT NOT NULL,
            text             TEXT NOT NULL,
            embedding        BLOB NOT NULL,
            source_session   TEXT,
            created_at       TEXT NOT NULL,
            last_recalled_at TEXT,
            recall_count     INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_memories_project ON memories(project_key);

        CREATE TABLE IF NOT EXISTS code_chunks (
            id          TEXT PRIMARY KEY,
            project_key TEXT NOT NULL,
            path        TEXT NOT NULL,
            start_line  INTEGER NOT NULL,
            end_line    INTEGER NOT NULL,
            text        TEXT NOT NULL,
            embedding   BLOB NOT NULL,
            updated_at  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_code_chunks_project ON code_chunks(project_key);

        CREATE TABLE IF NOT EXISTS session_chunks (
            id           TEXT PRIMARY KEY,
            project_key  TEXT NOT NULL,
            session_id   TEXT NOT NULL,
            turn_index   INTEGER NOT NULL,
            text         TEXT NOT NULL,
            embedding    BLOB NOT NULL,
            created_at   TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_session_chunks_project ON session_chunks(project_key);
        ",
    )?;

    for table in FTS_TABLES {
        create_fts(conn, table, "fts5")?;
    }

    Ok(())
}

/// Record tables with an FTS5 candidate index on their `text` column.
pub(crate) const FTS_TABLES: [&str; 3] = ["memories", "code_chunks", "session_chunks"];

/// External-content FTS5 index over `{table}.text`, kept in sync by triggers
/// so a row and its index entry land in the same transaction. Keyed by the
/// implicit rowid, which is stable because nothing here runs `VACUUM`.
///
/// Returns `false` when this SQLite build has no `module` (FTS5 compiled
/// out); search then falls back to a scan. A new index is backfilled once.
pub(crate) fn create_fts(
    conn: &Connection,
    table: &str,
    module: &str,
) -> Result<bool, rusqlite::Error> {
    let fts = format!("{table}_fts");
    let existed = table_exists(conn, &fts)?;
    // table/module names are internal constants only.
    let created = conn.execute_batch(&format!(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS {fts}
            USING {module}(text, content='{table}', content_rowid='rowid');
        CREATE TRIGGER IF NOT EXISTS {fts}_ai AFTER INSERT ON {table} BEGIN
            INSERT INTO {fts}(rowid, text) VALUES (new.rowid, new.text);
        END;
        CREATE TRIGGER IF NOT EXISTS {fts}_ad AFTER DELETE ON {table} BEGIN
            INSERT INTO {fts}({fts}, rowid, text) VALUES ('delete', old.rowid, old.text);
        END;
        CREATE TRIGGER IF NOT EXISTS {fts}_au AFTER UPDATE OF text ON {table} BEGIN
            INSERT INTO {fts}({fts}, rowid, text) VALUES ('delete', old.rowid, old.text);
            INSERT INTO {fts}(rowid, text) VALUES (new.rowid, new.text);
        END;
        "
    ));
    match created {
        Ok(()) => {}
        Err(e) if e.to_string().contains("no such module") => return Ok(false),
        Err(e) => return Err(e),
    }
    if !existed {
        conn.execute_batch(&format!("INSERT INTO {fts}({fts}) VALUES ('rebuild');"))?;
    }
    Ok(true)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 LIMIT 1")?;
    let mut rows = stmt.query(rusqlite::params![table])?;
    Ok(rows.next()?.is_some())
}

/// Add a column when missing. SQLite has no `ADD COLUMN IF NOT EXISTS`.
fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), rusqlite::Error> {
    if column_exists(conn, table, column)? {
        return Ok(());
    }
    // table/column names are internal constants only.
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
        [],
    ) {
        Ok(_) => Ok(()),
        // Race or pragma false-negative: treat as already migrated.
        Err(e) if e.to_string().contains("duplicate column") => Ok(()),
        Err(e) => Err(e),
    }
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    // Table-valued pragma is more reliable than iterating `PRAGMA table_info`
    // via a prepared statement on some rusqlite/SQLite combinations.
    let mut stmt = conn.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2 LIMIT 1")?;
    let mut rows = stmt.query(rusqlite::params![table, column])?;
    Ok(rows.next()?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_add_usage_columns_on_legacy_schema() {
        let conn = Connection::open_in_memory().unwrap();
        // Pre-usage schema (as first shipped).
        conn.execute_batch(
            "
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT,
                created_at TEXT,
                updated_at TEXT,
                project_path TEXT
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                role TEXT,
                content TEXT,
                tool_call_id TEXT,
                name TEXT,
                created_at TEXT
            );
            CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT);
            ",
        )
        .unwrap();

        run_migrations(&conn).unwrap();
        assert!(column_exists(&conn, "sessions", "input_tokens").unwrap());
        assert!(column_exists(&conn, "sessions", "output_tokens").unwrap());
        assert!(column_exists(&conn, "sessions", "cache_creation_input_tokens").unwrap());
        assert!(column_exists(&conn, "sessions", "cache_read_input_tokens").unwrap());

        // Idempotent.
        run_migrations(&conn).unwrap();
        assert!(column_exists(&conn, "memories", "embedding").unwrap());
        assert!(table_exists(&conn, "memories").unwrap());
    }

    fn fts_hits(conn: &Connection, table: &str, word: &str) -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table}_fts WHERE {table}_fts MATCH ?1"),
            [word],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn fts_backfills_existing_rows_once_and_tracks_writes() {
        let conn = Connection::open_in_memory().unwrap();
        // A pre-FTS database that already holds a fact.
        conn.execute_batch(
            "
            CREATE TABLE memories (
                id TEXT PRIMARY KEY, project_key TEXT NOT NULL, text TEXT NOT NULL,
                embedding BLOB NOT NULL, source_session TEXT, created_at TEXT NOT NULL,
                last_recalled_at TEXT, recall_count INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO memories (id, project_key, text, embedding, created_at)
                VALUES ('m1', 'p', 'bundled sqlite on windows', x'', 'now');
            ",
        )
        .unwrap();
        run_migrations(&conn).unwrap();
        assert_eq!(fts_hits(&conn, "memories", "bundled"), 1);
        // Idempotent: a second run does not duplicate index entries.
        run_migrations(&conn).unwrap();
        assert_eq!(fts_hits(&conn, "memories", "bundled"), 1);

        conn.execute(
            "UPDATE memories SET text = 'msvc linker' WHERE id = 'm1'",
            [],
        )
        .unwrap();
        assert_eq!(fts_hits(&conn, "memories", "bundled"), 0);
        assert_eq!(fts_hits(&conn, "memories", "linker"), 1);
        conn.execute("DELETE FROM memories WHERE id = 'm1'", [])
            .unwrap();
        assert_eq!(fts_hits(&conn, "memories", "linker"), 0);
    }

    #[test]
    fn create_fts_reports_a_missing_module_without_failing() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE notes (text TEXT NOT NULL);")
            .unwrap();
        assert!(!create_fts(&conn, "notes", "no_such_fts").unwrap());
        assert!(!table_exists(&conn, "notes_fts").unwrap());
        assert!(create_fts(&conn, "notes", "fts5").unwrap());
    }

    #[test]
    fn create_fts_propagates_other_errors() {
        let conn = Connection::open_in_memory().unwrap();
        // A trigger on a table that does not exist is not a missing module.
        assert!(create_fts(&conn, "missing", "fts5").is_err());
    }

    #[test]
    fn memories_table_created() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        assert!(table_exists(&conn, "memories").unwrap());
        run_migrations(&conn).unwrap();
    }

    #[test]
    fn messages_session_index_created() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_messages_session'",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        assert!(rows.next().unwrap().is_some());
        // Idempotent on legacy DBs that already had messages without the index.
        run_migrations(&conn).unwrap();
    }

    #[test]
    fn run_migrations_fails_when_first_batch_cannot_write() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA query_only=ON;").unwrap();
        assert!(run_migrations(&conn).is_err());
    }

    #[test]
    fn run_migrations_fails_adding_input_tokens_on_a_view() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE VIEW sessions AS
                SELECT 'x' AS id, 't' AS title, 'c' AS created_at,
                       'u' AS updated_at, 'p' AS project_path;
            CREATE TABLE messages (
                id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT,
                tool_call_id TEXT, name TEXT, created_at TEXT
            );
            CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT);
            ",
        )
        .unwrap();
        assert!(run_migrations(&conn).is_err());
    }

    #[test]
    fn run_migrations_fails_adding_output_tokens_at_column_limit() {
        let conn = Connection::open_in_memory().unwrap();
        let mut cols = vec![
            "id TEXT PRIMARY KEY".to_string(),
            "title TEXT".to_string(),
            "created_at TEXT".to_string(),
            "updated_at TEXT".to_string(),
            "project_path TEXT".to_string(),
            "input_tokens INTEGER NOT NULL DEFAULT 0".to_string(),
        ];
        for i in cols.len()..2000 {
            cols.push(format!("pad_{i} INTEGER"));
        }
        conn.execute(&format!("CREATE TABLE sessions ({})", cols.join(",")), [])
            .unwrap();
        conn.execute_batch(
            "
            CREATE TABLE messages (
                id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT,
                tool_call_id TEXT, name TEXT, created_at TEXT
            );
            CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT);
            ",
        )
        .unwrap();
        assert!(run_migrations(&conn).is_err());
    }

    #[test]
    fn run_migrations_fails_when_memory_index_cannot_be_created() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, title TEXT, created_at TEXT,
                updated_at TEXT, project_path TEXT
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT,
                tool_call_id TEXT, name TEXT, created_at TEXT
            );
            CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT);
            CREATE TABLE memories (id TEXT PRIMARY KEY);
            ",
        )
        .unwrap();
        assert!(run_migrations(&conn).is_err());
    }

    #[test]
    fn ensure_column_treats_duplicate_name_as_already_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (col INTEGER)").unwrap();
        // pragma_table_info is case-sensitive; SQLite column names are not.
        ensure_column(&conn, "t", "COL", "INTEGER").unwrap();
    }

    #[test]
    fn ensure_column_propagates_alter_errors() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(ensure_column(&conn, "missing", "col", "INTEGER").is_err());
    }
}
