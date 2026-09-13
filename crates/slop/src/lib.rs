//! Deterministic code-sloppiness metrics (ΔLOC, verbosity, erosion).
//!
//! No LLM-as-judge. Scores are tripwires, not training objectives — do not
//! RL against them. See `docs/guide.md` (Code slop).

mod git;
mod metrics;
mod parse;

use std::path::Path;

use serde::Serialize;

pub use parse::Lang;

/// Review / block tripwires. Defaults match issue #87 suggestions.
#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    pub verbosity: f64,
    pub erosion: f64,
    pub delta_loc: i64,
    pub block_verbosity: f64,
    pub block_erosion: f64,
    pub block_delta_loc: i64,
    pub hotspots: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            verbosity: 0.25,
            erosion: 0.50,
            delta_loc: 800,
            block_verbosity: 0.40,
            block_erosion: 0.75,
            block_delta_loc: 5_000,
            hotspots: 8,
        }
    }
}

impl Thresholds {
    pub fn from_parts(
        verbosity: f64,
        erosion: f64,
        delta_loc: i64,
        block_verbosity: f64,
        block_erosion: f64,
        block_delta_loc: i64,
        hotspots: usize,
    ) -> Self {
        Self {
            verbosity,
            erosion,
            delta_loc,
            block_verbosity,
            block_erosion,
            block_delta_loc,
            hotspots: hotspots.max(1),
        }
    }
}

/// `ok` until a review/block tripwire fires. Default product stance is review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Review,
    Block,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Review => "review",
            Self::Block => "block",
        }
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hotspot {
    pub path: String,
    pub function: String,
    pub cc: u32,
    pub sloc: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThresholdsOut {
    pub verbosity: f64,
    pub erosion: f64,
    pub delta_loc: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub base: String,
    pub head: String,
    pub delta_loc: i64,
    pub files_changed: usize,
    pub verbosity: f64,
    pub erosion: f64,
    pub hotspots: Vec<Hotspot>,
    pub verdict: Verdict,
    pub thresholds: ThresholdsOut,
    pub unparsed_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// One working-tree file to score (already filtered to the diff).
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}

/// Score an explicit file list. Used by tests and by [`analyze`] after git.
pub fn analyze_files(
    files: &[SourceFile],
    delta_loc: i64,
    files_changed: usize,
    base: &str,
    head: &str,
    thresholds: &Thresholds,
) -> Report {
    let scored = metrics::score_files(files, thresholds.hotspots);
    let verdict = verdict_of(scored.verbosity, scored.erosion, delta_loc, thresholds);
    Report {
        base: base.to_string(),
        head: head.to_string(),
        delta_loc,
        files_changed,
        verbosity: scored.verbosity,
        erosion: scored.erosion,
        hotspots: scored.hotspots,
        verdict,
        thresholds: ThresholdsOut {
            verbosity: thresholds.verbosity,
            erosion: thresholds.erosion,
            delta_loc: thresholds.delta_loc,
        },
        unparsed_files: scored.unparsed,
    }
}

/// Working tree vs `base` (merge-base with the default branch, else HEAD).
pub fn analyze(dir: &Path, base: Option<&str>, thresholds: &Thresholds) -> Result<Report, Error> {
    let snap = git::snapshot(dir, base)?;
    let files: Vec<SourceFile> = snap
        .contents
        .into_iter()
        .map(|(path, content)| SourceFile { path, content })
        .collect();
    Ok(analyze_files(
        &files,
        snap.delta_loc,
        snap.files_changed,
        &snap.base,
        &snap.head,
        thresholds,
    ))
}

pub fn format_report(report: &Report) -> String {
    let mut out = String::from("Slop\n");
    out.push_str(&format!("  base          {}\n", report.base));
    out.push_str(&format!("  head          {}\n", report.head));
    out.push_str(&format!("  ΔLOC          {}\n", report.delta_loc));
    out.push_str(&format!("  files         {}\n", report.files_changed));
    out.push_str(&format!("  verbosity     {:.3}\n", report.verbosity));
    out.push_str(&format!("  erosion       {:.3}\n", report.erosion));
    out.push_str(&format!("  verdict       {}\n", report.verdict));
    out.push_str(&format!(
        "  thresholds    verbosity {:.2} · erosion {:.2} · ΔLOC {}\n",
        report.thresholds.verbosity, report.thresholds.erosion, report.thresholds.delta_loc
    ));
    if !report.unparsed_files.is_empty() {
        out.push_str(&format!(
            "  unparsed      {}\n",
            report.unparsed_files.join(", ")
        ));
    }
    if report.hotspots.is_empty() {
        out.push_str("  hotspots      (none)\n");
    } else {
        out.push_str("  hotspots\n");
        for h in &report.hotspots {
            out.push_str(&format!(
                "    {}  {}  CC={} SLOC={}\n",
                h.path, h.function, h.cc, h.sloc
            ));
        }
    }
    out
}

pub fn language_for_path(path: &Path) -> Option<Lang> {
    parse::language_for_path(path)
}

pub(crate) fn verdict_of(verbosity: f64, erosion: f64, delta_loc: i64, t: &Thresholds) -> Verdict {
    if verbosity >= t.block_verbosity
        || erosion >= t.block_erosion
        || delta_loc >= t.block_delta_loc
    {
        Verdict::Block
    } else if verbosity >= t.verbosity || erosion >= t.erosion || delta_loc >= t.delta_loc {
        Verdict::Review
    } else {
        Verdict::Ok
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
