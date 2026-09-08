//! Language-server definitions: built-in defaults, user overlay, resolution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::detect;

/// One language server the agent can start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LspServerSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, alias = "fileTypes", skip_serializing_if = "Vec::is_empty")]
    pub file_types: Vec<String>,
    #[serde(default, alias = "languageId", skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    #[serde(default, alias = "rootMarkers", skip_serializing_if = "Vec::is_empty")]
    pub root_markers: Vec<String>,
    #[serde(
        default,
        alias = "initOptions",
        skip_serializing_if = "Option::is_none"
    )]
    pub init_options: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, alias = "isLinter", skip_serializing_if = "Option::is_none")]
    pub is_linter: Option<bool>,
}

impl LspServerSpec {
    pub fn is_disabled(&self) -> bool {
        self.disabled.unwrap_or(false)
    }

    pub fn is_linter(&self) -> bool {
        self.is_linter.unwrap_or(false)
    }

    pub fn handles_ext(&self, ext: &str) -> bool {
        let want = detect::normalize_ext(ext);
        self.file_types
            .iter()
            .any(|t| detect::normalize_ext(t) == want)
    }

    /// Higher-precedence `other` overlays this spec. Nested objects replace.
    pub fn overlay(&self, other: &LspServerSpec) -> LspServerSpec {
        LspServerSpec {
            command: other.command.clone().or_else(|| self.command.clone()),
            args: if other.args.is_empty() {
                self.args.clone()
            } else {
                other.args.clone()
            },
            file_types: if other.file_types.is_empty() {
                self.file_types.clone()
            } else {
                other.file_types.clone()
            },
            language_id: other
                .language_id
                .clone()
                .or_else(|| self.language_id.clone()),
            root_markers: if other.root_markers.is_empty() {
                self.root_markers.clone()
            } else {
                other.root_markers.clone()
            },
            init_options: other
                .init_options
                .clone()
                .or_else(|| self.init_options.clone()),
            settings: other.settings.clone().or_else(|| self.settings.clone()),
            disabled: other.disabled.or(self.disabled),
            is_linter: other.is_linter.or(self.is_linter),
        }
    }
}

/// Resolved command + spec ready to spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedServer {
    pub name: String,
    pub command: PathBuf,
    pub spec: LspServerSpec,
}

/// Built-in servers plus a user overlay (`config.toml` `[lsp]`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LspSettings {
    #[serde(
        default,
        alias = "idleTimeoutMs",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub servers: HashMap<String, LspServerSpec>,
}

impl LspSettings {
    /// Built-in auto-detect set. No user overlay.
    pub fn builtins() -> Self {
        let mut servers = HashMap::new();
        for (name, spec) in builtin_specs() {
            servers.insert(name.to_string(), spec);
        }
        Self {
            idle_timeout_ms: None,
            servers,
        }
    }

    /// Merge a user overlay onto the built-in set.
    ///
    /// Empty overlay servers keep auto-detect. Named servers overlay matching
    /// builtins (or register a new server). Nested objects replace.
    pub fn resolved(overlay: &LspSettings) -> Self {
        let mut out = Self::builtins();
        if overlay.idle_timeout_ms.is_some() {
            out.idle_timeout_ms = overlay.idle_timeout_ms;
        }
        for (name, spec) in &overlay.servers {
            match out.servers.get(name) {
                Some(base) => {
                    out.servers.insert(name.clone(), base.overlay(spec));
                }
                None => {
                    out.servers.insert(name.clone(), spec.clone());
                }
            }
        }
        out
    }

    /// First matching spec for `ext`, ignoring PATH and root markers.
    pub fn spec_for_ext(&self, ext: &str) -> Option<(&str, &LspServerSpec)> {
        self.ordered_candidates(ext).into_iter().next()
    }

    /// Pick a server for `ext` in `cwd`: root markers match, binary found.
    pub fn resolve(&self, ext: &str, cwd: &Path) -> Option<ResolvedServer> {
        for (name, spec) in self.ordered_candidates(ext) {
            if !detect::root_markers_match(cwd, &spec.root_markers) {
                continue;
            }
            let Some(cmd) = spec.command.as_deref() else {
                continue;
            };
            if let Some(resolved) = detect::resolve_command(cwd, cmd) {
                return Some(ResolvedServer {
                    name: name.to_string(),
                    command: resolved,
                    spec: spec.clone(),
                });
            }
        }
        None
    }

    fn ordered_candidates(&self, ext: &str) -> Vec<(&str, &LspServerSpec)> {
        let mut out: Vec<(&str, &LspServerSpec)> = self
            .servers
            .iter()
            .filter(|(_, spec)| !spec.is_disabled() && spec.handles_ext(ext))
            .map(|(n, s)| (n.as_str(), s))
            .collect();
        out.sort_by_key(|(name, spec)| {
            let builtin = BUILTIN_ORDER.iter().position(|n| n == name);
            (spec.is_linter(), builtin.unwrap_or(usize::MAX), *name)
        });
        out
    }
}

const BUILTIN_ORDER: &[&str] = &[
    "rust-analyzer",
    "clangd",
    "zls",
    "gopls",
    "typescript-language-server",
    "pyright",
    "pylsp",
    "jdtls",
    "omnisharp",
    "lua-language-server",
    "sourcekit-lsp",
    "bashls",
    "yaml-language-server",
    #[cfg(test)]
    "whycodes-lsp-fake",
    #[cfg(test)]
    "whycodes-lsp-empty",
    #[cfg(test)]
    "whycodes-lsp-failopen",
    #[cfg(test)]
    "whycodes-lsp-err",
    #[cfg(test)]
    "whycodes-lsp-missing",
    #[cfg(test)]
    "whycodes-lsp-nocmd",
];

fn spec(
    command: &str,
    args: &[&str],
    file_types: &[&str],
    language_id: Option<&str>,
    root_markers: &[&str],
) -> LspServerSpec {
    LspServerSpec {
        command: Some(command.to_string()),
        args: args.iter().map(|s| (*s).to_string()).collect(),
        file_types: file_types.iter().map(|s| (*s).to_string()).collect(),
        language_id: language_id.map(str::to_string),
        root_markers: root_markers.iter().map(|s| (*s).to_string()).collect(),
        init_options: None,
        settings: None,
        disabled: None,
        is_linter: None,
    }
}

fn builtin_specs() -> Vec<(&'static str, LspServerSpec)> {
    #[cfg(not(test))]
    {
        production_specs()
    }
    #[cfg(test)]
    {
        let mut out = production_specs();
        out.extend(test_fake_specs());
        out
    }
}

fn production_specs() -> Vec<(&'static str, LspServerSpec)> {
    vec![
        (
            "rust-analyzer",
            spec(
                "rust-analyzer",
                &[],
                &[".rs"],
                Some("rust"),
                &["Cargo.toml"],
            ),
        ),
        (
            "clangd",
            spec(
                "clangd",
                &[],
                &[".c", ".h", ".cpp", ".hpp", ".cc", ".cxx", ".hxx"],
                None,
                &["compile_commands.json", "CMakeLists.txt", ".clangd"],
            ),
        ),
        (
            "zls",
            spec("zls", &[], &[".zig"], Some("zig"), &["build.zig"]),
        ),
        (
            "gopls",
            spec("gopls", &[], &[".go"], Some("go"), &["go.mod"]),
        ),
        (
            "typescript-language-server",
            spec(
                "typescript-language-server",
                &["--stdio"],
                &[".ts", ".tsx", ".js", ".jsx"],
                None,
                &["package.json", "tsconfig.json", "jsconfig.json"],
            ),
        ),
        (
            "pyright",
            spec(
                "pyright-langserver",
                &["--stdio"],
                &[".py", ".pyi"],
                Some("python"),
                &[
                    "pyproject.toml",
                    "setup.py",
                    "setup.cfg",
                    "requirements.txt",
                    "Pipfile",
                ],
            ),
        ),
        (
            "pylsp",
            spec(
                "pylsp",
                &[],
                &[".py", ".pyi"],
                Some("python"),
                &[
                    "pyproject.toml",
                    "setup.py",
                    "setup.cfg",
                    "requirements.txt",
                    "Pipfile",
                ],
            ),
        ),
        (
            "jdtls",
            spec(
                "jdtls",
                &[],
                &[".java"],
                Some("java"),
                &["pom.xml", "build.gradle", "build.gradle.kts"],
            ),
        ),
        (
            "omnisharp",
            spec(
                "omnisharp",
                &["--languageserver"],
                &[".cs"],
                Some("csharp"),
                &["*.csproj", "*.sln"],
            ),
        ),
        (
            "lua-language-server",
            spec(
                "lua-language-server",
                &[],
                &[".lua"],
                Some("lua"),
                &[".luarc.json", ".luarc.jsonc"],
            ),
        ),
        (
            "sourcekit-lsp",
            spec(
                "sourcekit-lsp",
                &[],
                &[".swift"],
                Some("swift"),
                &["Package.swift"],
            ),
        ),
        (
            "bashls",
            spec(
                "bash-language-server",
                &["start"],
                &[".sh", ".bash"],
                Some("shellscript"),
                &[".bashrc", ".bash_profile"],
            ),
        ),
        (
            "yaml-language-server",
            spec(
                "yaml-language-server",
                &["--stdio"],
                &[".yaml", ".yml"],
                Some("yaml"),
                &[],
            ),
        ),
    ]
}

#[cfg(test)]
fn test_fake_specs() -> Vec<(&'static str, LspServerSpec)> {
    let py = crate::client::FAKE_LSP_PY;
    let python = crate::client::test_python();
    vec![
        (
            "whycodes-lsp-fake",
            spec(
                python,
                &["-c", py, "ok"],
                &[".whycodes_lsp_fake"],
                Some("rust"),
                &[],
            ),
        ),
        (
            "whycodes-lsp-empty",
            spec(
                python,
                &["-c", py, "empty"],
                &[".whycodes_lsp_empty"],
                Some("rust"),
                &[],
            ),
        ),
        (
            "whycodes-lsp-failopen",
            spec(
                python,
                &["-c", py, "init_then_eof"],
                &[".whycodes_lsp_failopen"],
                Some("rust"),
                &[],
            ),
        ),
        (
            "whycodes-lsp-err",
            spec(
                python,
                &["-c", py, "fail_after_open"],
                &[".whycodes_lsp_err"],
                Some("rust"),
                &[],
            ),
        ),
        (
            "whycodes-lsp-missing",
            spec(
                "whycodes-lsp-missing-bin",
                &[],
                &[".whycodes_lsp_missing"],
                Some("rust"),
                &[],
            ),
        ),
        (
            "whycodes-lsp-nocmd",
            LspServerSpec {
                command: None,
                file_types: vec![".whycodes_lsp_nocmd".into()],
                language_id: Some("rust".into()),
                ..LspServerSpec::default()
            },
        ),
    ]
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
