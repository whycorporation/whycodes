//! Run the project's own formatter on a file `edit` / `write` just changed.
//!
//! Only when the project shows it uses that formatter (a config file, or the
//! tool installed in `node_modules`), so an unformatted repo is never
//! reformatted wholesale. The formatter reads the file on stdin and prints
//! the result; nothing else on disk is touched (rustfmt on a path would also
//! rewrite the modules it declares). A missing binary, a syntax error or a
//! timeout leaves the file as the tool wrote it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use whycodes_core::types::ToolResult;

const TIMEOUT: Duration = Duration::from_secs(10);

const PRETTIER_BIN: &str = if cfg!(windows) {
    "prettier.cmd"
} else {
    "prettier"
};

const PRETTIER_CONFIGS: [&str; 13] = [
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.json5",
    ".prettierrc.yaml",
    ".prettierrc.yml",
    ".prettierrc.toml",
    ".prettierrc.js",
    ".prettierrc.cjs",
    ".prettierrc.mjs",
    "prettier.config.js",
    "prettier.config.cjs",
    "prettier.config.mjs",
    "prettier.config.ts",
];

/// A formatter that reads source on stdin and prints the formatted source.
#[derive(Debug, PartialEq)]
pub(crate) struct Formatter {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Working directory; config lookup starts here.
    pub dir: PathBuf,
}

/// Format the file a successful `edit` / `write` changed and say so in its
/// result. `path` is the tool's `path` argument.
pub(crate) async fn format_written_file(
    path: Option<&str>,
    working_dir: &str,
    result: &mut ToolResult,
) {
    let Some(path) = path.filter(|p| !p.is_empty() && !result.is_error) else {
        return;
    };
    let root = Path::new(working_dir);
    let file = root.join(path);
    let Some(formatter) = detect(&file, root) else {
        return;
    };
    let Ok(before) = std::fs::read_to_string(&file) else {
        return;
    };
    let Some(after) = run(&formatter, &before).await else {
        return;
    };
    if after == before || after.trim().is_empty() {
        return;
    }
    if let Err(e) = crate::file::atomic::write_atomic(&file, &after) {
        tracing::debug!(error = %e, file = %file.display(), "formatted output not written");
        return;
    }
    result.content.push_str(&format!(
        "\n\nFormatted with {}; line numbers and content tags may have moved.",
        formatter.name
    ));
}

/// The formatter this project uses for `file`, if any. Markers are looked up
/// from the file's directory up to `root`; a file outside `root` gets none.
pub(crate) fn detect(file: &Path, root: &Path) -> Option<Formatter> {
    let ext = file.extension()?.to_str()?.to_ascii_lowercase();
    let dir = file.parent()?.to_path_buf();
    let dirs: Vec<&Path> = dir
        .ancestors()
        .take_while(|d| d.starts_with(root))
        .collect();
    let shown = file.to_string_lossy().into_owned();
    let (name, program, args): (&'static str, PathBuf, Vec<String>) = match ext.as_str() {
        "rs" => {
            nearest(&dirs, &["rustfmt.toml", ".rustfmt.toml"])?;
            let mut args = vec!["--quiet".to_string()];
            if let Some(edition) = cargo_edition(&dirs) {
                args.extend(["--edition".to_string(), edition]);
            }
            ("rustfmt", "rustfmt".into(), args)
        }
        "go" => {
            nearest(&dirs, &["go.mod"])?;
            ("gofmt", "gofmt".into(), Vec::new())
        }
        "py" | "pyi" => {
            let stdin = vec!["--stdin-filename".to_string(), shown, "-".to_string()];
            if nearest(&dirs, &["ruff.toml", ".ruff.toml"]).is_some()
                || mentions(&dirs, "pyproject.toml", "[tool.ruff")
            {
                let args = ["format", "--quiet"].map(String::from).into_iter();
                ("ruff", "ruff".into(), args.chain(stdin).collect())
            } else if mentions(&dirs, "pyproject.toml", "[tool.black") {
                (
                    "black",
                    "black".into(),
                    [vec!["--quiet".to_string()], stdin].concat(),
                )
            } else {
                return None;
            }
        }
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc" | "css"
        | "scss" | "less" | "html" | "vue" | "md" | "mdx" | "yaml" | "yml" | "graphql" => {
            if nearest(&dirs, &PRETTIER_CONFIGS).is_none()
                && !mentions(&dirs, "package.json", "\"prettier\"")
            {
                return None;
            }
            let bin = dirs
                .iter()
                .map(|d| d.join("node_modules").join(".bin").join(PRETTIER_BIN))
                .find(|p| p.is_file())?;
            ("prettier", bin, vec!["--stdin-filepath".to_string(), shown])
        }
        _ => return None,
    };
    Some(Formatter {
        name,
        program,
        args,
        dir,
    })
}

/// The nearest of `names` in `dirs` (nearest directory first).
fn nearest(dirs: &[&Path], names: &[&str]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

/// Whether the nearest file called `name` contains `needle`.
fn mentions(dirs: &[&Path], name: &str, needle: &str) -> bool {
    nearest(dirs, &[name])
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|text| text.contains(needle))
}

/// `edition = "…"` from the nearest `Cargo.toml` that sets it (a member that
/// inherits it falls through to the workspace root).
fn cargo_edition(dirs: &[&Path]) -> Option<String> {
    dirs.iter()
        .filter_map(|d| std::fs::read_to_string(d.join("Cargo.toml")).ok())
        .find_map(|text| {
            text.lines().find_map(|line| {
                let value = line.trim().strip_prefix("edition")?.trim_start();
                let value = value.strip_prefix('=')?.trim().trim_matches('"');
                Some(value.to_string())
            })
        })
}

/// Formatted `input`, or `None` when the formatter is missing, fails, times
/// out, or prints something that is not UTF-8.
pub(crate) async fn run(formatter: &Formatter, input: &str) -> Option<String> {
    let mut child = match tokio::process::Command::new(&formatter.program)
        .args(&formatter.args)
        .current_dir(&formatter.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            tracing::debug!(error = %e, formatter = formatter.name, "formatter not started");
            return None;
        }
    };
    let mut stdin = child.stdin.take()?;
    let input = input.to_string();
    // Write from a task: a large file would fill stdout while we still write.
    tokio::spawn(async move { stdin.write_all(input.as_bytes()).await });
    let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
#[path = "formatter_tests.rs"]
mod tests;
