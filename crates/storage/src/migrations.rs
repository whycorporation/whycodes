use rusqlite::Connection;

/// Run all migrations on the given connection, creating tables
/// if they do not already exist.
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
    Ok(())
}
