//! Session list/export and stats.
use super::helpers::*;
use crate::args::*;
use colored::*;
use std::path::PathBuf;
use whycodes_config::Config;

pub(crate) async fn cmd_session(cmd: &SessionCmd) -> anyhow::Result<()> {
    let db = open_db()?;

    match cmd {
        SessionCmd::List => {
            let sessions = db
                .list_sessions()
                .map_err(|e| anyhow::anyhow!("Failed to list sessions: {}", e))?;

            if sessions.is_empty() {
                println!("{} No sessions found.", "ℹ".cyan());
                println!("Start a session with: whycodes run");
            } else {
                println!("{} Sessions:", "📋".bold());
                for s in &sessions {
                    let msg_count = db.message_count(&s.id).unwrap_or(0);
                    let mut title = s.title.clone();
                    // Backfill legacy placeholders so the list is scannable.
                    if msg_count > 0
                        && whycodes_session::title::looks_like_default_title(
                            &title,
                            std::path::Path::new(&s.project_path),
                        )
                        && let Ok(Some(mut loaded)) =
                            whycodes_session::session::Session::load_from_db(&db, &s.id)
                        && loaded.maybe_upgrade_title_from_history()
                    {
                        if let Err(err) = loaded.save_to_db(&db) {
                            tracing::warn!(
                                error = %err,
                                "failed to persist backfilled session title"
                            );
                        }
                        title = loaded.title;
                    }
                    println!("  {} — {} ({} messages)", s.id.cyan(), title, msg_count);
                    println!("    Created: {}  Updated: {}", s.created_at, s.updated_at);
                    if !s.project_path.is_empty() && s.project_path != "/" {
                        println!("    Project: {}", s.project_path);
                    }
                }
            }
        }
        SessionCmd::View { id } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    let msg_count = db.message_count(&s.id).unwrap_or(0);
                    println!("{} Session: {}", "📋".bold(), s.id.cyan());
                    println!("  Title:     {}", s.title);
                    println!("  Created:   {}", s.created_at);
                    println!("  Updated:   {}", s.updated_at);
                    println!("  Messages:  {}", msg_count);
                    println!("  Project:   {}", s.project_path);

                    // Show recent messages
                    let messages = db.get_messages(id).unwrap_or_default();
                    if !messages.is_empty() {
                        println!("  --- Messages ---");
                        for msg in messages.iter().rev().take(10).rev() {
                            println!(
                                "    [{}] {}: {}",
                                msg.role,
                                msg.created_at,
                                truncate_str(&msg.content, 120)
                            );
                        }
                    }
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Delete { id } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    db.delete_session(id)?;
                    println!(
                        "{} Session '{}' ({}) deleted.",
                        "✓".green(),
                        id.cyan(),
                        s.title
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Rename { id, name } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    let cleaned = whycodes_session::sanitize_title(name);
                    if cleaned.is_empty() {
                        eprintln!("{} Empty title after sanitize.", "✗".red());
                        return Ok(());
                    }
                    db.update_title(id, &cleaned)?;
                    println!(
                        "{} Session '{}' renamed from '{}' to '{}'.",
                        "✓".green(),
                        id.cyan(),
                        s.title,
                        cleaned.cyan()
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Import { path, from } => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
            let kind = whycodes_session::ImportKind::parse(from);
            let messages = whycodes_session::import_messages(&raw, kind)?;
            let project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let title = path.file_stem().and_then(|s| s.to_str());
            let session = whycodes_session::Session::from_imported(project, messages, title);
            session.save_to_db(&db)?;
            println!(
                "{} Imported {} messages as session {}",
                "✓".green(),
                session.messages.len(),
                session.id.cyan()
            );
            println!("Resume with: whycodes --resume {}", &session.id[..8]);
        }
        SessionCmd::Share { id } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    let messages = db.get_messages(id).unwrap_or_default();
                    let share_data = serde_json::json!({
                        "session": {
                            "id": s.id,
                            "title": s.title,
                            "created_at": s.created_at,
                            "updated_at": s.updated_at,
                            "project_path": s.project_path,
                        },
                        "messages": messages.iter().map(|m| {
                            serde_json::json!({
                                "id": m.id,
                                "role": m.role,
                                "content": m.content,
                                "created_at": m.created_at,
                            })
                        }).collect::<Vec<_>>(),
                    });

                    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
                    let shares_dir = data_dir.join("shares");
                    std::fs::create_dir_all(&shares_dir)?;
                    let share_path = shares_dir.join(format!("{}.json", id));

                    let json = serde_json::to_string_pretty(&share_data)?;
                    std::fs::write(&share_path, &json)?;

                    println!(
                        "{} Session exported to: {}",
                        "✓".green(),
                        share_path.display().to_string().cyan()
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
    }

    Ok(())
}

/// `stats` — Show usage statistics (provider-reported tokens when available)
pub(crate) async fn cmd_stats() -> anyhow::Result<()> {
    // A missing database is the normal state before the first session, and is
    // not worth an error. Anything else — a locked file, a permission problem —
    // is reported rather than hidden behind "no database found", which is what
    // previously made a database fault indistinguishable from a fresh install.
    let db = match open_db() {
        Ok(d) => d,
        Err(e) if is_missing_database(&e) => {
            println!("{} No statistics database found.", "ℹ".cyan());
            println!("Stats are collected as you use whycodes.");
            return Ok(());
        }
        Err(e) => {
            println!(
                "{} Could not open the statistics database: {e}",
                "!".yellow()
            );
            return Ok(());
        }
    };

    let totals = match db.usage_totals() {
        Ok(t) => t,
        Err(e) => {
            println!("{} Could not read usage totals: {e}", "!".yellow());
            return Ok(());
        }
    };

    println!("{} Usage Statistics:", "📊".bold());
    println!("  Sessions:  {}", totals.session_count);
    println!("  Messages:  {}", totals.message_count);

    if totals.usage.is_empty() {
        println!("  Tokens:    (none recorded yet)");
        println!(
            "  {}",
            "Token totals appear after sessions that report provider usage.".dimmed()
        );
    } else {
        println!(
            "  Tokens:    {} total ({} in + {} out)",
            totals.usage.total(),
            totals.usage.input_tokens,
            totals.usage.output_tokens
        );
        if let Some(read) = totals.usage.cache_read_input_tokens {
            println!(
                "  Cache:     {} read, {} write",
                read,
                totals.usage.cache_creation_input_tokens.unwrap_or(0)
            );
        } else if let Some(write) = totals.usage.cache_creation_input_tokens {
            println!("  Cache:     {} write", write);
        }
    }

    // Top sessions by total tokens (when any usage is stored).
    if !totals.usage.is_empty() {
        let mut sessions = db.list_sessions().unwrap_or_default();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.usage.total()));
        let top: Vec<_> = sessions
            .into_iter()
            .filter(|s| !s.usage.is_empty())
            .take(5)
            .collect();
        if !top.is_empty() {
            println!();
            println!("  Top sessions by tokens:");
            for s in top {
                let title = if s.title.is_empty() {
                    s.id.chars().take(8).collect::<String>()
                } else {
                    s.title.clone()
                };
                println!("    {:>8}  {}  {}", s.usage.total(), title, s.project_path);
            }
        }
    }

    if totals.session_count > 0 {
        let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
        let db_path = data_dir.join("whycodes.db");
        if let Ok(meta) = std::fs::metadata(&db_path) {
            println!();
            println!("  DB size:   {} bytes", meta.len());
        }
    }

    Ok(())
}

// ────────────────────────────────────────────────────────────────────────
// Auth (OAuth subscription login)
// ────────────────────────────────────────────────────────────────────────
