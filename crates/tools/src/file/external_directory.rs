use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct ExternalDirectoryTool;

impl Default for ExternalDirectoryTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalDirectoryTool {
    pub fn new() -> Self {
        Self
    }

    /// Check if a path is allowed for external access.
    /// Reads the .whycodes/external_dirs_allowed file (relative to `working_dir`)
    /// and checks if the given path (or any of its parent directories) is listed.
    fn is_path_allowed(path: &str, working_dir: &str) -> bool {
        let allowed_file = whycodes_core::project_dir(std::path::Path::new(working_dir))
            .join("external_dirs_allowed");

        let allowed_content = match std::fs::read_to_string(&allowed_file) {
            Ok(content) => content,
            Err(e) => {
                tracing::debug!(path = %allowed_file.display(), error = %e, "external dirs allowlist unreadable; denying");
                return false;
            }
        };

        let canon_path = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(path = %path, error = %e, "external path canonicalize failed; denying");
                return false;
            }
        };

        for line in allowed_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let allowed = match std::fs::canonicalize(line) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(line = %line, error = %e, "allowlist entry canonicalize failed; skipping");
                    continue;
                }
            };
            // Check if the requested path is equal to or a child of an allowed directory
            if canon_path == allowed || canon_path.starts_with(&allowed) {
                return true;
            }
        }

        false
    }
}
impl Tool for ExternalDirectoryTool {
    fn name(&self) -> &str {
        "external_directory"
    }

    fn description(&self) -> &str {
        "Access files outside the project directory (requires permission)"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the external file or directory"
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform: 'read' to read a file, 'list' to list a directory",
                    "enum": ["read", "list"]
                }
            },
            "required": ["path", "action"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_str = args["path"].as_str().unwrap_or("");
            let action = args["action"].as_str().unwrap_or("read");

            let full_path = if std::path::Path::new(path_str).is_absolute() {
                path_str.to_string()
            } else {
                std::path::Path::new(&ctx.working_dir)
                    .join(path_str)
                    .to_string_lossy()
                    .to_string()
            };

            // Security check: verify the path is allowed
            if !Self::is_path_allowed(&full_path, &ctx.working_dir) {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!(
                        "Access denied: '{}' is not in the allowed external directories list. \
                     Add the directory to .whycodes/external_dirs_allowed to grant access.",
                        path_str
                    ),
                    is_error: true,
                };
            }

            match action {
                "list" => {
                    let entries = match std::fs::read_dir(&full_path) {
                        Ok(entries) => entries,
                        Err(e) => {
                            return ToolResult {
                                tool_call_id: String::new(),
                                content: format!("Error listing directory '{}': {}", full_path, e),
                                is_error: true,
                            };
                        }
                    };

                    let mut output = String::new();
                    for entry in entries {
                        match entry {
                            Ok(e) => {
                                let meta = match e.metadata() {
                                    Ok(m) => m,
                                    Err(err) => {
                                        tracing::debug!(error = %err, "entry metadata failed; skipping");
                                        continue;
                                    }
                                };
                                let name = e.file_name().to_string_lossy().to_string();
                                let file_type = if meta.is_dir() {
                                    "d"
                                } else if meta.is_symlink() {
                                    "l"
                                } else {
                                    "-"
                                };
                                let size = meta.len();
                                output.push_str(&format!(
                                    "{:<10} {:>10} {}\n",
                                    file_type, size, name
                                ));
                            }
                            Err(err) => {
                                tracing::debug!(error = %err, "dir entry read failed; skipping");
                                continue;
                            }
                        }
                    }

                    ToolResult {
                        tool_call_id: String::new(),
                        content: if output.is_empty() {
                            format!("Directory '{}' is empty", full_path)
                        } else {
                            output
                        },
                        is_error: false,
                    }
                }
                "read" => match std::fs::read_to_string(&full_path) {
                    Ok(content) => ToolResult {
                        tool_call_id: String::new(),
                        content,
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error reading file '{}': {}", full_path, e),
                        is_error: true,
                    },
                },
                _ => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Unknown action '{}'. Use 'read' or 'list'.", action),
                    is_error: true,
                },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    fn allow(dir: &std::path::Path, lines: &str) {
        let why = dir.join(".whycodes");
        std::fs::create_dir_all(&why).expect("mkdir .whycodes");
        std::fs::write(why.join("external_dirs_allowed"), lines).expect("write allowlist");
    }

    #[tokio::test]
    async fn metadata_describes_external_directory_tool() {
        let t = ExternalDirectoryTool::new();
        assert_eq!(t.name(), "external_directory");
        assert!(t.description().contains("outside"));
        let params = t.parameters();
        assert_eq!(params["required"][0], "path");
        assert_eq!(params["properties"]["action"]["enum"][0], "read");
    }

    #[tokio::test]
    async fn denies_when_allowlist_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("secret.txt"), "nope").expect("write");

        let out = ExternalDirectoryTool::new()
            .execute(
                json!({ "path": outside.path().join("secret.txt").to_string_lossy(), "action": "read" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Access denied"), "{}", out.content);
    }

    #[tokio::test]
    async fn read_and_list_when_path_is_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("note.txt"), "hello").expect("write");
        std::fs::create_dir(outside.path().join("sub")).expect("mkdir");
        allow(
            dir.path(),
            &format!("# comment\n\n{}\n", outside.path().display()),
        );

        let read = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().join("note.txt").to_string_lossy(),
                    "action": "read"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!read.is_error, "{}", read.content);
        assert_eq!(read.content, "hello");

        let list = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().to_string_lossy(),
                    "action": "list"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!list.is_error, "{}", list.content);
        assert!(list.content.contains("note.txt"), "{}", list.content);
        assert!(list.content.contains("sub"), "{}", list.content);
        assert!(list.content.contains("d"), "{}", list.content);
    }

    #[tokio::test]
    async fn unknown_action_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        allow(dir.path(), &format!("{}\n", outside.path().display()));

        let out = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().to_string_lossy(),
                    "action": "wipe"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Unknown action"), "{}", out.content);
    }

    #[tokio::test]
    async fn list_on_a_file_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("f.txt"), "x").expect("write");
        allow(dir.path(), &format!("{}\n", outside.path().display()));

        let out = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().join("f.txt").to_string_lossy(),
                    "action": "list"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Error listing directory"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn read_on_a_directory_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        allow(dir.path(), &format!("{}\n", outside.path().display()));

        // Missing paths fail canonicalize and are denied; a directory is
        // allowed but cannot be read as text.
        let out = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().to_string_lossy(),
                    "action": "read"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Error reading file"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn relative_path_resolves_from_working_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("ext");
        std::fs::create_dir(&nested).expect("mkdir");
        std::fs::write(nested.join("a.txt"), "rel").expect("write");
        allow(dir.path(), &format!("{}\n", nested.display()));

        let out = ExternalDirectoryTool::new()
            .execute(
                json!({ "path": "ext/a.txt", "action": "read" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "rel");
    }

    #[tokio::test]
    async fn empty_directory_list_is_labelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        allow(dir.path(), &format!("{}\n", outside.path().display()));

        let out = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().to_string_lossy(),
                    "action": "list"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("is empty"), "{}", out.content);
    }

    #[tokio::test]
    async fn default_action_is_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("a.txt"), "def").expect("write");
        allow(dir.path(), &format!("{}\n", outside.path().display()));

        let out = ExternalDirectoryTool::new()
            .execute(
                json!({ "path": outside.path().join("a.txt").to_string_lossy() }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "def");
    }

    #[tokio::test]
    async fn default_constructs() {
        assert_eq!(ExternalDirectoryTool.name(), "external_directory");
    }

    #[tokio::test]
    async fn deny_missing_path_and_broken_allowlist_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        allow(
            dir.path(),
            &format!("/nonexistent-xyz-allow\n{}\n", outside.path().display()),
        );
        let missing = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": outside.path().join("gone.txt").to_string_lossy(),
                    "action": "read"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(missing.is_error, "{}", missing.content);
        assert!(
            missing.content.contains("Access denied"),
            "{}",
            missing.content
        );

        let other = tempfile::tempdir().expect("other");
        std::fs::write(other.path().join("x.txt"), "nope").unwrap();
        let denied = ExternalDirectoryTool::new()
            .execute(
                json!({
                    "path": other.path().join("x.txt").to_string_lossy(),
                    "action": "read"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(denied.is_error, "{}", denied.content);
    }
}
