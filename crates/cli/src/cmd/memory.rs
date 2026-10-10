//! Memory subcommands.
use super::helpers::*;
use crate::Cli;
use crate::args::*;
use colored::*;
use whycodes_config::Config;

pub(crate) async fn cmd_memory(cli: &Cli, cmd: &MemoryCmd) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load_or_create())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let svc = open_memory_service(cli, &config)?;

    match cmd {
        MemoryCmd::List { limit } => {
            let rows = svc.list(*limit)?;
            if rows.is_empty() {
                println!("{}", memory_empty_line());
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
            let page = svc.search_page(query, *limit, config.memory.recall_min_score.min(0.15))?;
            println!("{}", whycodes_tools::cards::memory_cards(query, &page));
        }
        MemoryCmd::Add { text } => {
            let text = text.join(" ");
            if text.trim().is_empty() {
                anyhow::bail!("usage: whycodes memory add <text>");
            }
            let id = svc.remember(&text, None)?;
            println!("{}", memory_saved_line(&id, &text));
        }
        MemoryCmd::Delete { id } => {
            if svc.delete(id)? {
                println!("{}", memory_deleted_line(id));
            } else {
                println!("{}", memory_delete_missing_line(id));
            }
        }
        MemoryCmd::Clear => {
            let n = svc.clear()?;
            println!("{}", memory_cleared_line(n));
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
                    println!("{}", memory_exported_line(&path.display().to_string()));
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
            println!("{}", memory_indexing_line());
            let n = svc.index_codebase(*max_files, *max_chunks)?;
            println!("{}", memory_indexed_line(n));
        }
        MemoryCmd::SessionSearch { query, limit } => {
            let page =
                svc.search_sessions_page(query, *limit, config.memory.session_min_score.min(0.1))?;
            println!("{}", whycodes_tools::cards::session_cards(query, &page));
        }
        MemoryCmd::CodeSearch { query, limit } => {
            let page =
                svc.search_code_page(query, *limit, config.memory.code_min_score.min(0.1))?;
            println!("{}", whycodes_tools::cards::code_cards(query, &page));
        }
        MemoryCmd::OnnxSmoke => {
            if !whycodes_memory::onnx::onnx_available() {
                anyhow::bail!(
                    "ONNX not in this binary. Rebuild with: cargo build -p whycodes-cli --features onnx"
                );
            }
            let data_dir = Config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
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

pub(crate) fn memory_empty_line() -> String {
    format!("{} No memories for this project.", "ℹ".cyan())
}

pub(crate) fn memory_saved_line(id: &str, text: &str) -> String {
    format!(
        "{} Saved {} — {text}",
        "✓".green(),
        id.chars().take(8).collect::<String>().cyan()
    )
}

pub(crate) fn memory_deleted_line(id: &str) -> String {
    format!("{} Deleted {id}", "✓".green())
}

pub(crate) fn memory_delete_missing_line(id: &str) -> String {
    format!("{} No memory matching '{id}'", "ℹ".cyan())
}

pub(crate) fn memory_cleared_line(n: usize) -> String {
    format!("{} Cleared {n} memories", "✓".green())
}

pub(crate) fn memory_exported_line(path: &str) -> String {
    format!("{} Exported to {path}", "✓".green())
}

pub(crate) fn memory_indexing_line() -> String {
    format!("{} Indexing codebase…", "⚡".bold())
}

pub(crate) fn memory_indexed_line(n: usize) -> String {
    format!("{} Indexed {n} code chunks", "✓".green())
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

pub(crate) fn memory_import_summary(added: usize, skipped: usize) -> String {
    format!("Import complete: {added} added, {skipped} skipped")
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
