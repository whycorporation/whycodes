//! Memory subcommands.
use super::helpers::*;
use crate::Cli;
use crate::args::*;
use colored::*;
use whycodes_config::Config;

pub(crate) async fn cmd_memory(cli: &Cli, cmd: &MemoryCmd) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let svc = open_memory_service(cli, &config)?;

    match cmd {
        MemoryCmd::List { limit } => {
            let rows = svc.list(*limit)?;
            if rows.is_empty() {
                println!("{} No memories for this project.", "ℹ".cyan());
            } else {
                println!(
                    "{} {}",
                    "🧠".bold(),
                    memory_list_header(rows.len(), &svc.project_key)
                );
                for r in rows {
                    println!("{}", memory_row_line(&r.id, &r.text));
                }
            }
        }
        MemoryCmd::Search { query, limit } => {
            let hits = svc.search(query, *limit, config.memory.recall_min_score.min(0.15))?;
            if hits.is_empty() {
                println!("{} No matches.", "ℹ".cyan());
            } else {
                for h in hits {
                    println!(
                        "{}",
                        memory_search_line(h.score, &h.entry.id, &h.entry.text)
                    );
                }
            }
        }
        MemoryCmd::Add { text } => {
            let text = text.join(" ");
            if text.trim().is_empty() {
                anyhow::bail!("usage: whycodes memory add <text>");
            }
            let id = svc.remember(&text, None)?;
            println!(
                "{} Saved {} — {}",
                "✓".green(),
                id.chars().take(8).collect::<String>().cyan(),
                text
            );
        }
        MemoryCmd::Delete { id } => {
            if svc.delete(id)? {
                println!("{} Deleted {id}", "✓".green());
            } else {
                println!("{} No memory matching '{id}'", "ℹ".cyan());
            }
        }
        MemoryCmd::Clear => {
            let n = svc.clear()?;
            println!("{} Cleared {n} memories", "✓".green());
        }
        MemoryCmd::Path => {
            println!("{}", svc.memory_md_path().display());
            println!(
                "{} project_key={} bank={} scope={} backend={} enabled={} onnx_build={}",
                "ℹ".dimmed(),
                svc.project_key,
                svc.bank_key,
                config.memory.scope,
                config.memory.embed_backend,
                config.memory.enabled,
                whycodes_memory::onnx::onnx_available()
            );
        }
        MemoryCmd::Export { output } => {
            let json = svc.export_json()?;
            match output {
                Some(path) => {
                    std::fs::write(path, &json)?;
                    println!("{} Exported to {}", "✓".green(), path.display());
                }
                None => println!("{json}"),
            }
        }
        MemoryCmd::Import { path } => {
            let json = std::fs::read_to_string(path)?;
            let (added, skipped) = svc.import_json(&json)?;
            println!("{} {}", "✓".green(), memory_import_summary(added, skipped));
        }
        MemoryCmd::Index {
            max_files,
            max_chunks,
        } => {
            println!("{} Indexing codebase…", "⚡".bold());
            let n = svc.index_codebase(*max_files, *max_chunks)?;
            println!("{} Indexed {n} code chunks", "✓".green());
        }
        MemoryCmd::SessionSearch { query, limit } => {
            let hits =
                svc.search_sessions(query, *limit, config.memory.session_min_score.min(0.1))?;
            if hits.is_empty() {
                println!(
                    "{} No session hits yet. They appear after turns are retained.",
                    "ℹ".cyan()
                );
            } else {
                for h in hits {
                    println!(
                        "{}",
                        memory_session_hit_line(h.score, &h.entry.session_id, h.entry.turn_index)
                    );
                    for line in h.entry.text.lines().take(4) {
                        println!("      {}", line.dimmed());
                    }
                }
            }
        }
        MemoryCmd::CodeSearch { query, limit } => {
            let hits = svc.search_code(query, *limit, config.memory.code_min_score.min(0.1))?;
            if hits.is_empty() {
                println!(
                    "{} No code hits. Run `whycodes memory index` first.",
                    "ℹ".cyan()
                );
            } else {
                for h in hits {
                    println!(
                        "{}",
                        memory_code_hit_line(
                            h.score,
                            &h.entry.path,
                            h.entry.start_line,
                            h.entry.end_line
                        )
                    );
                    for line in h.entry.text.lines().take(4) {
                        println!("      {}", line.dimmed());
                    }
                }
            }
        }
        MemoryCmd::OnnxSmoke => {
            if !whycodes_memory::onnx::onnx_available() {
                anyhow::bail!(
                    "ONNX not in this binary. Rebuild with: cargo build -p whycodes-cli --features onnx"
                );
            }
            let data_dir = Config::data_dir()?;
            println!(
                "{} Running ONNX smoke (download + checksum + embed)…",
                "⚡".bold()
            );
            let (dim, norm) = whycodes_memory::onnx::smoke_embed(&data_dir)?;
            println!(
                "{} OK — embedding dim={dim}, L2-norm={norm:.4} (≈1.0 expected)",
                "✓".green()
            );
            println!(
                "  model dir: {}",
                whycodes_memory::onnx::model_dir(&data_dir).display()
            );
        }
    }
    Ok(())
}

pub(crate) fn memory_list_header(count: usize, project_key: &str) -> String {
    format!("{count} memories ({project_key})")
}

pub(crate) fn memory_row_line(id: &str, text: &str) -> String {
    format!(
        "  {}  {text}",
        id.chars().take(8).collect::<String>().dimmed()
    )
}

pub(crate) fn memory_search_line(score: f32, id: &str, text: &str) -> String {
    format!(
        "  [{:.2}] {}  {text}",
        score,
        id.chars().take(8).collect::<String>().dimmed()
    )
}

pub(crate) fn memory_session_hit_line(score: f32, session_id: &str, turn_index: i64) -> String {
    format!(
        "  [{:.2}] {} turn {turn_index}",
        score,
        &session_id[..8.min(session_id.len())]
    )
}

pub(crate) fn memory_code_hit_line(
    score: f32,
    path: &str,
    start_line: i64,
    end_line: i64,
) -> String {
    format!("  [{score:.2}] {path}:{start_line}-{end_line}")
}

pub(crate) fn memory_import_summary(added: usize, skipped: usize) -> String {
    format!("Import complete: {added} added, {skipped} skipped")
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
