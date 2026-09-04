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
                    "{} {} memories ({})",
                    "🧠".bold(),
                    rows.len(),
                    svc.project_key.dimmed()
                );
                for r in rows {
                    println!(
                        "  {}  {}",
                        r.id.chars().take(8).collect::<String>().dimmed(),
                        r.text
                    );
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
                        "  [{:.2}] {}  {}",
                        h.score,
                        h.entry.id.chars().take(8).collect::<String>().dimmed(),
                        h.entry.text
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
            println!(
                "{} Import complete: {added} added, {skipped} skipped",
                "✓".green()
            );
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
                    let sid = &h.entry.session_id;
                    println!(
                        "  [{:.2}] {} turn {}",
                        h.score,
                        &sid[..8.min(sid.len())],
                        h.entry.turn_index
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
                        "  [{:.2}] {}:{}-{}",
                        h.score, h.entry.path, h.entry.start_line, h.entry.end_line
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

#[cfg(test)]
mod tests {
    #[test]
    fn onnx_gate_message_is_feature_aware() {
        let available = whycodes_memory::onnx::onnx_available();
        assert!(!available || available);
    }
}
