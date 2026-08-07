//! MEMORY.md index — always-on inject + dual-write with SQLite.

use std::path::Path;

use chrono::Utc;

/// Load MEMORY.md, capped to `max_lines` lines or `max_bytes` bytes (whichever first).
/// Returns empty string if missing or empty after trim.
pub fn load_capped(path: &Path, max_lines: usize, max_bytes: usize) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    cap_content(&raw, max_lines, max_bytes)
}

fn cap_content(raw: &str, max_lines: usize, max_bytes: usize) -> String {
    let mut out = String::new();
    let mut lines = 0usize;
    for line in raw.lines() {
        let candidate_len = out.len() + line.len() + 1;
        if lines >= max_lines || candidate_len > max_bytes {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        lines += 1;
    }
    out.trim().to_string()
}

/// Append a fact line to MEMORY.md (creates file with header if needed).
pub fn append_entry(path: &Path, id: &str, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let date = Utc::now().format("%Y-%m-%d");
    let line = format!("- [{date}] {} (id:{id})\n", text.trim());

    if path.exists() {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
        f.write_all(line.as_bytes())?;
    } else {
        let body = format!("# Whycode auto memory\n\n{line}");
        std::fs::write(path, body)?;
    }
    Ok(())
}

/// Remove the line containing `(id:{id})` from MEMORY.md.
pub fn remove_entry(path: &Path, id: &str) -> std::io::Result<bool> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let marker = format!("(id:{id})");
    let mut kept = Vec::new();
    let mut removed = false;
    for line in raw.lines() {
        if line.contains(&marker) {
            removed = true;
            continue;
        }
        kept.push(line);
    }
    if !removed {
        return Ok(false);
    }
    let mut body = kept.join("\n");
    if !body.ends_with('\n') && !body.is_empty() {
        body.push('\n');
    }
    std::fs::write(path, body)?;
    Ok(true)
}

/// Truncate MEMORY.md to header only (or delete if no header needed).
pub fn clear_file(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::write(path, "# Whycode auto memory\n\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cap_stops_at_line_limit() {
        let raw = (0..10)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = cap_content(&raw, 3, 10_000);
        assert_eq!(capped.lines().count(), 3);
    }

    #[test]
    fn append_and_remove() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("MEMORY.md");
        append_entry(&path, "abc", "prefer pnpm").unwrap();
        append_entry(&path, "def", "use rustfmt").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("prefer pnpm"));
        assert!(body.contains("(id:abc)"));
        assert!(remove_entry(&path, "abc").unwrap());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("prefer pnpm"));
        assert!(body.contains("use rustfmt"));
    }
}
