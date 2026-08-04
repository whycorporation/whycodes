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

    Ok(())
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
    }
}
