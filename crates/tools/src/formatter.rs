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

const BIOME_BIN: &str = if cfg!(windows) { "biome.cmd" } else { "biome" };

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
    // Tools that read their config only from the working directory run where
    // the marker is.
    let mut cwd = None;
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
            // A Biome or Deno config names the formatter outright; Prettier
            // is the fallback for everything else.
            let biome_ext = matches!(
                ext.as_str(),
                "js" | "jsx"
                    | "mjs"
                    | "cjs"
                    | "ts"
                    | "tsx"
                    | "mts"
                    | "cts"
                    | "json"
                    | "jsonc"
                    | "css"
            );
            let deno_ext = matches!(
                ext.as_str(),
                "js" | "jsx" | "ts" | "tsx" | "json" | "jsonc" | "md"
            );
            if biome_ext && nearest(&dirs, &["biome.json", "biome.jsonc"]).is_some() {
                let bin = node_bin(&dirs, BIOME_BIN).unwrap_or_else(|| "biome".into());
                let path_arg = format!("--stdin-file-path={shown}");
                ("biome", bin, vec!["format".to_string(), path_arg])
            } else if deno_ext && nearest(&dirs, &["deno.json", "deno.jsonc"]).is_some() {
                let args = vec!["fmt".to_string(), format!("--ext={ext}"), "-".to_string()];
                ("deno", "deno".into(), args)
            } else {
                if nearest(&dirs, &PRETTIER_CONFIGS).is_none()
                    && !mentions(&dirs, "package.json", "\"prettier\"")
                {
                    return None;
                }
                let bin = node_bin(&dirs, PRETTIER_BIN)?;
                ("prettier", bin, vec!["--stdin-filepath".to_string(), shown])
            }
        }
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "m" | "mm" | "proto" => {
            nearest(&dirs, &[".clang-format", "_clang-format"])?;
            let args = vec![format!("--assume-filename={shown}")];
            ("clang-format", "clang-format".into(), args)
        }
        "zig" => {
            nearest(&dirs, &["build.zig"])?;
            (
                "zig fmt",
                "zig".into(),
                vec!["fmt".into(), "--stdin".into()],
            )
        }
        "tf" | "tfvars" => {
            let initialized = dirs.iter().any(|d| d.join(".terraform").is_dir());
            if !initialized && nearest(&dirs, &[".terraform.lock.hcl"]).is_none() {
                return None;
            }
            (
                "terraform fmt",
                "terraform".into(),
                vec!["fmt".into(), "-".into()],
            )
        }
        "lua" => {
            nearest(&dirs, &["stylua.toml", ".stylua.toml"])?;
            let args = vec!["--stdin-filepath".to_string(), shown, "-".to_string()];
            ("stylua", "stylua".into(), args)
        }
        "toml" => {
            let config = nearest(&dirs, &["taplo.toml", ".taplo.toml"])?;
            cwd = config.parent().map(Path::to_path_buf);
            ("taplo", "taplo".into(), vec!["fmt".into(), "-".into()])
        }
        "ex" | "exs" | "heex" => {
            let config = nearest(&dirs, &[".formatter.exs"])?;
            cwd = config.parent().map(Path::to_path_buf);
            let args = ["format", "--stdin-filename"].map(String::from).into_iter();
            (
                "mix format",
                "mix".into(),
                args.chain([shown, "-".into()]).collect(),
            )
        }
        _ => return None,
    };
    Some(Formatter {
        name,
        program,
        args,
        dir: cwd.unwrap_or(dir),
    })
}

/// `node_modules/.bin/<bin>` in the nearest directory that has it.
fn node_bin(dirs: &[&Path], bin: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|d| d.join("node_modules").join(".bin").join(bin))
        .find(|p| p.is_file())
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
