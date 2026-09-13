//! `whycodes slop` — ΔLOC / verbosity / erosion on the working-tree diff.

use super::helpers::*;
use crate::Cli;
use whycodes_config::Config;

pub(crate) async fn cmd_slop(cli: &Cli, base: Option<&str>, json: bool) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let config = Config::load_layered(&project_dir)?;
    let thresholds = slop_thresholds(&config.slop);
    match whycodes_slop::analyze(&project_dir, base, &thresholds) {
        Ok(report) => {
            if json {
                serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
                println!();
            } else {
                print!("{}", whycodes_slop::format_report(&report));
            }
            Ok(())
        }
        Err(e) => {
            if json {
                let v = serde_json::json!({
                    "error": e.to_string(),
                    "verdict": "review",
                });
                serde_json::to_writer_pretty(std::io::stdout().lock(), &v)?;
                println!();
            } else {
                eprintln!("whycodes slop: {e}");
            }
            Err(anyhow::anyhow!("{e}"))
        }
    }
}

#[cfg(test)]
#[path = "slop_tests.rs"]
mod tests;
