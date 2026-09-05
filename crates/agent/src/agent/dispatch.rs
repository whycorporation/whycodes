//! In-process tool dispatch: background jobs, tool_search, worktree, schedule, swarm, task.

use std::sync::Arc;

use whycodes_core::tool::ToolContext;
use whycodes_core::types::{ToolCall, ToolResult};
use whycodes_session::session::Session;
use whycodes_tools::profile::ToolProfile;

use crate::events::{EventSink, TurnEvent, emit};
use crate::subagent::{SubagentRunner, SubagentTask};
use crate::tool_policy::*;

use super::{Agent, persist_agent_artifact};

impl Agent {
    /// `bash`/`shell` with `background: true` — return job id immediately.
    pub(crate) fn execute_background_shell(
        &self,
        call: &ToolCall,
        tool_ctx: &ToolContext,
        events: Option<&EventSink>,
    ) -> ToolResult {
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if command.trim().is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "background shell requires a non-empty `command`".into(),
                is_error: true,
            };
        }
        let label = call
            .arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // Prefer long-lived sink; fall back to turn events for listener (optional).
        if self.event_sink.is_none()
            && let Some(tx) = events
        {
            let tx = tx.clone();
            self.background
                .set_listener(Some(std::sync::Arc::new(move |ev| {
                    let _ = tx.send(TurnEvent::Background {
                        id: ev.id,
                        status: ev.status.as_str().to_string(),
                        summary: ev.summary,
                    });
                })));
        }
        match self.background.start_shell(
            &command,
            std::path::PathBuf::from(&tool_ctx.working_dir),
            tool_ctx.sandbox.clone(),
            label,
        ) {
            Ok(id) => {
                emit(
                    &events.cloned().or_else(|| self.event_sink.clone()),
                    TurnEvent::Background {
                        id: id.clone(),
                        status: "running".into(),
                        summary: truncate_permission_detail(&command),
                    },
                );
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!(
                        "Background job `{id}` started.\n\
                         Command: {command}\n\
                         Use tool `bg` with action=list|read|kill (id={id})."
                    ),
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: call.id.clone(),
                content: e,
                is_error: true,
            },
        }
    }

    pub(crate) fn execute_bg_tool(&self, call: &ToolCall) -> ToolResult {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("list");
        match action {
            "list" => {
                let jobs = self.background.list();
                if jobs.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "No background jobs.".into(),
                        is_error: false,
                    };
                }
                let mut lines = vec![format!(
                    "Background jobs ({} running):",
                    self.background.running_count()
                )];
                for j in jobs {
                    lines.push(format!(
                        "- {} [{}] {:.1}s · {}{}",
                        j.id,
                        j.status.as_str(),
                        j.elapsed.as_secs_f64(),
                        j.label,
                        j.exit_code
                            .map(|c| format!(" exit={c}"))
                            .unwrap_or_default()
                    ));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "read" => {
                let id = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if id.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "bg read requires `id`".into(),
                        is_error: true,
                    };
                }
                let max = call
                    .arguments
                    .get("max_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(8_000) as usize;
                match self.background.read(id, max) {
                    Ok(s) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: s,
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            "kill" => {
                let id = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if id.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "bg kill requires `id`".into(),
                        is_error: true,
                    };
                }
                match self.background.kill(id) {
                    Ok(s) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: s,
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            other => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!("unknown bg action `{other}` (list|read|kill)"),
                is_error: true,
            },
        }
    }

    pub(crate) fn execute_tool_search(&self, call: &ToolCall) -> ToolResult {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("search");
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let max = call
            .arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .clamp(1, 40) as usize;

        let catalog = self.tool_executor.deferred_catalog(&self.info.permission);
        let activated = self.activated_tools_snapshot();

        match action {
            "list" => {
                let mut lines = vec![format!(
                    "Activated ({}): {}",
                    activated.len(),
                    if activated.is_empty() {
                        "(none)".into()
                    } else {
                        activated.join(", ")
                    }
                )];
                lines.push(format!("Deferred catalogue ({}):", catalog.len()));
                for (name, desc) in catalog.iter().take(40) {
                    let active = if activated.iter().any(|a| a == name) {
                        " [on]"
                    } else {
                        ""
                    };
                    let short: String = desc.chars().take(72).collect();
                    lines.push(format!("- {name}{active} — {short}"));
                }
                if catalog.len() > 40 {
                    lines.push(format!("…and {} more", catalog.len() - 40));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "select" => {
                if query.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "tool_search select requires `query` (tool name or comma list)"
                            .into(),
                        is_error: true,
                    };
                }
                let names: Vec<String> = query
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                let mut added = Vec::new();
                let mut missing = Vec::new();
                let mut guard = self
                    .activated_tools
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                for name in names {
                    if self.tool_executor.get(&name).is_some() {
                        if !ToolProfile::Core.includes(&name) {
                            guard.insert(name.clone());
                        }
                        added.push(name);
                    } else {
                        missing.push(name);
                    }
                }
                drop(guard);
                let mut content = format!(
                    "Activated for this session: {}\nThey appear on the next LLM step.",
                    if added.is_empty() {
                        "(none)".into()
                    } else {
                        added.join(", ")
                    }
                );
                if !missing.is_empty() {
                    content.push_str(&format!("\nUnknown: {}", missing.join(", ")));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content,
                    is_error: !missing.is_empty() && added.is_empty(),
                }
            }
            _ => {
                if query.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "tool_search search requires `query` keywords".into(),
                        is_error: true,
                    };
                }
                let q = query.to_ascii_lowercase();
                let terms: Vec<&str> = q.split_whitespace().collect();
                let mut scored: Vec<(i32, &str, &str)> = catalog
                    .iter()
                    .filter_map(|(name, desc)| {
                        let hay = format!("{name} {desc}").to_ascii_lowercase();
                        let mut score = 0i32;
                        for t in &terms {
                            if name.to_ascii_lowercase() == *t {
                                score += 10;
                            } else if name.to_ascii_lowercase().contains(t) {
                                score += 5;
                            } else if hay.contains(t) {
                                score += 2;
                            }
                        }
                        if score > 0 {
                            Some((score, name.as_str(), desc.as_str()))
                        } else {
                            None
                        }
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
                scored.truncate(max);
                if scored.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "No deferred tools match `{query}`. Try action=list for the catalogue."
                        ),
                        is_error: false,
                    };
                }
                let mut lines = vec![format!(
                    "Matches for `{query}` (select with action=select query=<name>):"
                )];
                for (_, name, desc) in scored {
                    let short: String = desc.chars().take(80).collect();
                    lines.push(format!("- {name} — {short}"));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
        }
    }

    pub(crate) fn execute_worktree_tool(&self, call: &ToolCall, session: &Session) -> ToolResult {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = call
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let root = crate::swarm_worktree::git_toplevel(&session.project_path)
            .unwrap_or_else(|| session.project_path.clone());
        let base = whycodes_core::project_dir(&root).join("worktrees");

        match action {
            "list" => {
                let mut lines = vec![format!("Worktrees under {}", base.display())];
                if let Some(cwd) = self.cwd_override_path() {
                    lines.push(format!("Active cwd override: {}", cwd.display()));
                }
                match std::fs::read_dir(&base) {
                    Ok(rd) => {
                        let mut names: Vec<_> = rd
                            .flatten()
                            .filter(|e| e.path().is_dir())
                            .map(|e| e.file_name().to_string_lossy().to_string())
                            .collect();
                        names.sort();
                        if names.is_empty() {
                            lines.push("(none)".into());
                        } else {
                            for n in names {
                                lines.push(format!("- {n}"));
                            }
                        }
                    }
                    Err(_read_dir) => lines.push("(directory missing — create one first)".into()),
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "create" => {
                if name.is_empty() || !is_safe_worktree_name(name) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "worktree create needs a safe `name` (alnum, -, _)".into(),
                        is_error: true,
                    };
                }
                if !crate::swarm_worktree::is_git_repo(&root) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "not a git repository".into(),
                        is_error: true,
                    };
                }
                let dest = base.join(name);
                match crate::swarm_worktree::create_worktree(&root, &dest, name) {
                    Ok(wt) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "Created worktree `{name}` at {}\nbase HEAD {}\nUse action=enter name={name} to switch tool cwd.",
                            wt.path.display(),
                            wt.base_head
                        ),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            "remove" => {
                if name.is_empty() || !is_safe_worktree_name(name) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "worktree remove needs a safe `name`".into(),
                        is_error: true,
                    };
                }
                let dest = base.join(name);
                if let Ok(mut g) = self.cwd_override.lock()
                    && g.as_ref().is_some_and(|p| p.starts_with(&dest))
                {
                    *g = None;
                }
                let wt = crate::swarm_worktree::SwarmWorktree {
                    path: dest,
                    repo_root: root,
                    base_head: String::new(),
                    worker_id: name.to_string(),
                };
                match crate::swarm_worktree::remove_worktree(&wt) {
                    Ok(()) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!("Removed worktree `{name}`"),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            "enter" => {
                if name.is_empty() || !is_safe_worktree_name(name) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "worktree enter needs a safe `name`".into(),
                        is_error: true,
                    };
                }
                let dest = base.join(name);
                if !dest.is_dir() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "worktree `{name}` not found — create it first ({})",
                            dest.display()
                        ),
                        is_error: true,
                    };
                }
                if let Ok(mut g) = self.cwd_override.lock() {
                    *g = Some(dest.clone());
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!(
                        "Tool cwd → {}\nUse action=exit to restore project root.",
                        dest.display()
                    ),
                    is_error: false,
                }
            }
            "exit" => {
                let prev = self.cwd_override.lock().ok().and_then(|mut g| g.take());
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: match prev {
                        Some(p) => {
                            format!("Restored tool cwd to project root (was {})", p.display())
                        }
                        None => "No worktree cwd override was active.".into(),
                    },
                    is_error: false,
                }
            }
            other => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!(
                    "unknown worktree action `{other}` (create|list|remove|enter|exit)"
                ),
                is_error: true,
            },
        }
    }

    /// Delay then either start a background shell or enqueue a user prompt.
    pub(crate) async fn execute_schedule_tool(
        &self,
        call: &ToolCall,
        tool_ctx: &ToolContext,
        events: Option<&EventSink>,
    ) -> ToolResult {
        let after_secs = call
            .arguments
            .get("after_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(86_400);
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let goal = call
            .arguments
            .get("goal")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if command.is_none() && goal.is_none() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "schedule requires `command` and/or `goal`".into(),
                is_error: true,
            };
        }

        let background = self.background.clone();
        let sandbox = tool_ctx.sandbox.clone();
        let cwd = std::path::PathBuf::from(&tool_ctx.working_dir);
        let sink = events.cloned().or_else(|| self.event_sink.clone());
        let label = call
            .arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        tokio::spawn(async move {
            if after_secs > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(after_secs)).await;
            }
            if let Some(cmd) = command {
                match background.start_shell(&cmd, cwd, sandbox, label) {
                    Ok(id) => {
                        if let Some(ref tx) = sink {
                            let _ = tx.send(TurnEvent::Background {
                                id: id.clone(),
                                status: "running".into(),
                                summary: format!("scheduled: {cmd}"),
                            });
                        }
                    }
                    Err(e) => {
                        if let Some(ref tx) = sink {
                            let _ = tx.send(TurnEvent::Background {
                                id: "schedule".into(),
                                status: "failed".into(),
                                summary: e,
                            });
                        }
                    }
                }
            }
            if let Some(g) = goal
                && let Some(ref tx) = sink
            {
                let _ = tx.send(TurnEvent::EnqueuePrompt { text: g });
            }
        });

        let mut parts = vec![format!("Scheduled in {after_secs}s")];
        if let Some(ref c) = call.arguments.get("command").and_then(|v| v.as_str()) {
            parts.push(format!("shell: {c}"));
        }
        if let Some(ref g) = call.arguments.get("goal").and_then(|v| v.as_str()) {
            parts.push(format!("prompt queue: {g}"));
        }
        ToolResult {
            tool_call_id: call.id.clone(),
            content: parts.join("\n"),
            is_error: false,
        }
    }

    /// Execute the `swarm` tool: parallel subagents + file-claim / worktree isolation.
    pub(crate) async fn execute_swarm_tool(
        &self,
        call: &ToolCall,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        events: Option<&EventSink>,
    ) -> whycodes_core::types::ToolResult {
        use std::time::Instant;
        use tokio::sync::Semaphore;
        use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet, ToolResult};
        use whycodes_core::{ClaimResult, FileClaimRegistry};

        if !self.swarm_enabled {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "swarm is disabled (`[swarm] enabled = false` in config).".into(),
                is_error: true,
            };
        }

        let specs = match crate::swarm::parse_swarm_tasks(&call.arguments) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    tool_call_id: call.id.clone(),
                    content: e,
                    is_error: true,
                };
            }
        };

        let max_from_args = call
            .arguments
            .get("max_concurrent")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let max_concurrent = max_from_args
            .unwrap_or(self.swarm_max_agents)
            .clamp(1, crate::swarm::SWARM_HARD_MAX_AGENTS)
            .min(specs.len())
            .max(1);

        // Worktrees when configured and project is a git repo; else same-checkout + claims.
        let repo_root = crate::swarm_worktree::git_toplevel(&session.project_path)
            .unwrap_or_else(|| session.project_path.clone());
        let use_worktrees =
            self.swarm_worktrees && crate::swarm_worktree::is_git_repo(&session.project_path);
        let run_id = format!(
            "{}-{}",
            session.id.chars().take(8).collect::<String>(),
            chrono::Utc::now().format("%H%M%S")
        );
        let swarm_run_dir = crate::swarm_worktree::run_dir(&repo_root, &run_id);

        let claims = FileClaimRegistry::new();
        let hub = whycodes_core::SwarmHub::new();
        hub.ensure("parent");
        if let Some(tx) = events {
            let tx_c = tx.clone();
            claims.set_listener(Some(std::sync::Arc::new(move |ev| {
                if let Err(e) = tx_c.send(TurnEvent::FileConflict {
                    path: ev.path,
                    claimant: ev.claimant_label,
                    owner: ev.owner_label,
                }) {
                    tracing::debug!(error = %e, "swarm conflict event dropped");
                }
            })));
            let tx_s = tx.clone();
            claims.set_stale_listener(Some(std::sync::Arc::new(move |ev| {
                if let Err(e) = tx_s.send(TurnEvent::FileStale {
                    path: ev.path,
                    reader: ev.reader_id,
                    writer: ev.writer_label,
                }) {
                    tracing::debug!(error = %e, "swarm stale event dropped");
                }
            })));
            let tx_m = tx.clone();
            hub.set_listener(Some(std::sync::Arc::new(move |msg| {
                if let Err(e) = tx_m.send(TurnEvent::SwarmMessage {
                    from: msg.from,
                    to: msg.to,
                    text: msg.text,
                }) {
                    tracing::debug!(error = %e, "swarm message event dropped");
                }
            })));
        }

        let wall_t0 = Instant::now();
        let total = specs.len();
        let mode_label = if use_worktrees {
            "worktrees"
        } else if self.swarm_worktrees {
            "same-checkout (not a git repo)"
        } else {
            "same-checkout (worktrees off)"
        };
        emit(
            &events.cloned(),
            TurnEvent::SwarmStatus {
                active: 0,
                total,
                message: format!(
                    "Starting swarm: {total} workers, {mode_label}, max {max_concurrent} concurrent…"
                ),
            },
        );

        // Pre-claim optional paths (logical ownership for merge / same-checkout).
        for (i, spec) in specs.iter().enumerate() {
            let worker_id = format!("worker-{i}");
            let label = format!("{worker_id}/{}", spec.subagent_type);
            for rel in &spec.paths {
                let full = if std::path::Path::new(rel).is_absolute() {
                    std::path::PathBuf::from(rel)
                } else {
                    // Claim against main checkout paths so ownership is shared across worktrees.
                    session.project_path.join(rel)
                };
                match claims.try_claim(&worker_id, &label, &full) {
                    ClaimResult::Acquired | ClaimResult::Held => {}
                    ClaimResult::Conflict {
                        owner_label,
                        owner_id: _,
                    } => {
                        if let Some(tx) = events {
                            let _ = tx.send(TurnEvent::FileConflict {
                                path: full.display().to_string(),
                                claimant: label.clone(),
                                owner: owner_label.clone(),
                            });
                        }
                        return ToolResult {
                            tool_call_id: call.id.clone(),
                            content: format!(
                                "Pre-claim conflict: `{rel}` for {label} is already claimed by `{owner_label}`. \
                                 Give each worker disjoint `paths`."
                            ),
                            is_error: true,
                        };
                    }
                }
            }
        }

        let sem = std::sync::Arc::new(Semaphore::new(max_concurrent));
        let (worker_provider, worker_model) =
            crate::routing::resolve_worker_model(provider_name, model, self.model_smol.as_deref());
        let provider_name: std::sync::Arc<str> = worker_provider.into();
        let model: std::sync::Arc<str> = worker_model.into();
        let api_key: std::sync::Arc<str> = api_key.into();
        let project_path = session.project_path.clone();
        let registry = Arc::clone(&self.provider_registry);
        let executor = Arc::clone(&self.tool_executor);
        let sandbox = self.sandbox.clone();
        let network = self.network.clone();
        let memory = self.memory.clone();
        let parent_permission = self.info.permission.clone();
        let agents_md_path = session.project_path.clone();
        let repo_root_arc = repo_root.clone();
        let swarm_run_dir = swarm_run_dir.clone();
        let file_index = self.file_index.clone();
        let panel = self.panel_sink();
        let hub = hub.clone();

        let mut handles = Vec::with_capacity(specs.len());

        for (i, spec) in specs.into_iter().enumerate() {
            let worker_id = format!("worker-{i}");
            let label = format!("{worker_id}/{}", spec.subagent_type);
            let permit = Arc::clone(&sem);
            let pn = Arc::clone(&provider_name);
            let m = Arc::clone(&model);
            let ak = Arc::clone(&api_key);
            let claims = claims.clone();
            let registry = Arc::clone(&registry);
            let executor = Arc::clone(&executor);
            let sandbox = sandbox.clone();
            let network = network.clone();
            let memory = memory.clone();
            let parent_permission = parent_permission.clone();
            let project_path = project_path.clone();
            let agents_md_path = agents_md_path.clone();
            let events_tx = events.cloned();
            let repo_root = repo_root_arc.clone();
            let swarm_run_dir = swarm_run_dir.clone();
            let file_index = file_index.clone();
            let panel = panel.clone();
            let hub = hub.clone();
            hub.ensure(&worker_id);

            handles.push(tokio::spawn(async move {
                let _guard = match permit.acquire().await {
                    Ok(g) => g,
                    Err(_closed) => {
                        return (
                            worker_id,
                            spec.subagent_type,
                            spec.goal,
                            false,
                            0.0,
                            "Semaphore closed".to_string(),
                            whycodes_core::types::Usage::default(),
                            None,
                            project_path,
                            events_tx,
                            label,
                        );
                    }
                };
                if let Some(ref tx) = events_tx {
                    let _ = tx.send(TurnEvent::SwarmStatus {
                        active: 0,
                        total,
                        message: format!("Swarm {label}: running…"),
                    });
                }

                // Optional isolated checkout.
                let mut worktree = None;
                let worker_cwd = if use_worktrees {
                    let dest = swarm_run_dir.join(&worker_id);
                    match crate::swarm_worktree::create_worktree(&repo_root, &dest, &worker_id) {
                        Ok(wt) => {
                            let path = wt.path.clone();
                            worktree = Some(wt);
                            path
                        }
                        Err(e) => {
                            return (
                                worker_id,
                                spec.subagent_type,
                                spec.goal,
                                false,
                                0.0,
                                format!("Failed to create git worktree: {e}"),
                                whycodes_core::types::Usage::default(),
                                None,
                                project_path,
                                events_tx,
                                label,
                            );
                        }
                    }
                } else {
                    project_path.clone()
                };

                let (permission, system_prompt) = match spec.subagent_type.as_str() {
                    "explore" | "scout" => (
                        PermissionSet {
                            allowed_tools: Some(vec![
                                "read".into(),
                                "grep".into(),
                                "glob".into(),
                                "list".into(),
                                "webfetch".into(),
                                "websearch".into(),
                                "lsp".into(),
                                "swarm_msg".into(),
                            ]),
                            denied_tools: Some(vec![
                                "write".into(),
                                "edit".into(),
                                "shell".into(),
                                "bash".into(),
                                "apply_patch".into(),
                                "todowrite".into(),
                                "todo".into(),
                                "task".into(),
                                "swarm".into(),
                            ]),
                            allow_file_writes: false,
                            allow_network: true,
                            allow_shell: false,
                            allowed_paths: None,
                            rules: Default::default(),
                        },
                        Agent::system_prompt_for(&spec.subagent_type),
                    ),
                    _ => {
                        let mut perm = parent_permission;
                        let mut denied = perm.denied_tools.unwrap_or_default();
                        for t in ["todowrite", "todo", "todoread", "task", "swarm"] {
                            if !denied.iter().any(|x| x == t) {
                                denied.push(t.to_string());
                            }
                        }
                        perm.denied_tools = Some(denied);
                        (perm, Agent::system_prompt_for("general"))
                    }
                };

                let isolation_note = if worktree.is_some() {
                    "\n\nYou are running in an isolated git worktree. Edit freely; \
                     changes merge back into the main checkout when you finish. \
                     Prefer staying within your assigned paths."
                        .to_string()
                } else {
                    "\n\nYou share the main checkout with sibling workers. \
                     File claims block double-writes. Use `swarm_msg` to tell \
                     siblings what you changed. A `read` of a file another \
                     worker wrote will be marked stale."
                        .to_string()
                };
                let claim_note = if spec.paths.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nYou own these paths exclusively for this swarm run: {}.\
                         \nDo not edit other workers' files.",
                        spec.paths.join(", ")
                    )
                };
                let context = {
                    let extra = format!("{isolation_note}{claim_note}");
                    match spec.context {
                        Some(c) if extra.is_empty() => Some(c),
                        Some(c) => Some(format!("{c}{extra}")),
                        None if extra.is_empty() => None,
                        None => Some(extra.trim().to_string()),
                    }
                };

                let info = AgentInfo {
                    name: spec.subagent_type.clone(),
                    description: format!("Swarm worker {worker_id}"),
                    mode: AgentMode::Subagent,
                    permission,
                    model: None,
                    system_prompt: Some(Agent::with_agents_md(&system_prompt, &agents_md_path)),
                    temperature: None,
                    top_p: None,
                };

                let task = SubagentTask {
                    goal: spec.goal.clone(),
                    context,
                    tools: None,
                    max_turns: spec.max_turns,
                };

                // File claims apply in same-checkout mode; with worktrees,
                // physical isolation holds during the run and merge does 3-way.
                let mut runner =
                    SubagentRunner::new(registry, executor, info, worker_cwd, sandbox, network)
                        .with_memory(memory)
                        .with_file_index(file_index.clone())
                        .with_panel(panel.clone())
                        .with_swarm_hub(Some(hub.clone()));
                if !use_worktrees {
                    runner =
                        runner.with_file_claims(claims.clone(), worker_id.clone(), label.clone());
                }

                let t0 = Instant::now();
                if let Some(ref tx) = events_tx
                    && let Err(e) = tx.send(TurnEvent::Subagent {
                        id: worker_id.clone(),
                        kind: spec.subagent_type.clone(),
                        description: spec.goal.clone(),
                        status: "running".into(),
                        activity: "Thinking".into(),
                        elapsed_ms: 0,
                        output: String::new(),
                    })
                {
                    tracing::debug!(error = %e, "subagent running event dropped");
                }
                let result = runner.run(task, &pn, &m, &ak).await;
                let secs = t0.elapsed().as_secs_f64();
                claims.release_agent(&worker_id);

                let (success, body, worker_usage) = match result {
                    Ok(r) => (r.success, r.output, r.usage),
                    Err(e) => (
                        false,
                        format!("Swarm worker error: {e}"),
                        whycodes_core::types::Usage::default(),
                    ),
                };
                if success {
                    persist_agent_artifact(&project_path, &worker_id, &body);
                }
                if let Some(ref tx) = events_tx
                    && let Err(e) = tx.send(TurnEvent::Subagent {
                        id: worker_id.clone(),
                        kind: spec.subagent_type.clone(),
                        description: spec.goal.clone(),
                        status: if success { "completed" } else { "failed" }.into(),
                        activity: String::new(),
                        elapsed_ms: (secs * 1000.0) as u64,
                        output: body.clone(),
                    })
                {
                    tracing::debug!(error = %e, "subagent finished event dropped");
                }

                (
                    worker_id,
                    spec.subagent_type,
                    spec.goal,
                    success,
                    secs,
                    body,
                    worker_usage,
                    worktree,
                    project_path,
                    events_tx,
                    label,
                )
            }));
        }

        let mut sections = Vec::with_capacity(handles.len());
        let mut ok = 0usize;
        let mut merge_conflicts = 0usize;
        for handle in handles {
            match handle.await {
                Ok((
                    worker_id,
                    kind,
                    goal,
                    mut success,
                    secs,
                    mut body,
                    worker_usage,
                    worktree,
                    project_path,
                    events_tx,
                    label,
                )) => {
                    if !worker_usage.is_empty()
                        && let Ok(mut pending) = self.subagent_usage_pending.lock()
                    {
                        pending.add(&worker_usage);
                    }
                    if let Some(wt) = worktree {
                        let merge = crate::swarm_worktree::merge_into_main(&wt, &project_path);
                        for c in &merge.conflicts {
                            if let Some(ref tx) = events_tx {
                                let _ = tx.send(TurnEvent::FileConflict {
                                    path: c.path.clone(),
                                    claimant: label.clone(),
                                    owner: "main".into(),
                                });
                            }
                        }
                        if !merge.conflicts.is_empty() {
                            success = false;
                        }
                        let merge_txt = crate::swarm_worktree::format_merge_report(&merge);
                        if !merge_txt.is_empty() {
                            body = format!("{body}\n\n{merge_txt}");
                        }
                        if let Err(e) = crate::swarm_worktree::remove_worktree(&wt) {
                            body = format!("{body}\n\n_Worktree cleanup warning: {e}_");
                        }
                    }

                    if success {
                        ok += 1;
                    }
                    if body.contains("**Merge conflicts:**") {
                        merge_conflicts += 1;
                    }
                    sections.push(crate::swarm::format_worker_report(
                        &worker_id, &kind, success, secs, &goal, &body,
                    ));
                }
                Err(e) => {
                    sections.push(format!("### worker join error\n\n{e}\n"));
                }
            }
        }

        claims.clear();
        // Best-effort prune empty swarm run dir.
        let _ = std::fs::remove_dir_all(&swarm_run_dir);

        let wall = wall_t0.elapsed().as_secs_f64();
        emit(
            &events.cloned(),
            TurnEvent::SwarmStatus {
                active: 0,
                total,
                message: format!(
                    "Swarm done: {ok}/{total} ok in {wall:.1}s ({mode_label}{})",
                    if merge_conflicts > 0 {
                        format!(", {merge_conflicts} merge conflict(s)")
                    } else {
                        String::new()
                    }
                ),
            },
        );

        let mut report = crate::swarm::format_swarm_header(total, ok, wall);
        report.push_str(&format!("\n_isolation: {mode_label}_\n\n"));
        report.push_str(&sections.join("\n"));

        ToolResult {
            tool_call_id: call.id.clone(),
            content: report,
            is_error: ok == 0,
        }
    }

    /// Execute the `task` tool by spawning a real subagent (OpenCode Task tool parity).
    pub(crate) async fn execute_task_tool(
        &self,
        call: &ToolCall,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        events: Option<&EventSink>,
    ) -> whycodes_core::types::ToolResult {
        use whycodes_core::types::{AgentMode, PermissionSet, ToolResult};

        let goal = call
            .arguments
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if goal.is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "task requires a non-empty `goal`".to_string(),
                is_error: true,
            };
        }

        let context = call
            .arguments
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let subagent_type = call
            .arguments
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        let max_turns = call
            .arguments
            .get("max_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(15) as usize;

        // Permission profile per OpenCode subagent type
        let (permission, system_prompt) = match subagent_type {
            "explore" | "scout" => (
                PermissionSet {
                    allowed_tools: Some(vec![
                        "read".into(),
                        "grep".into(),
                        "glob".into(),
                        "list".into(),
                        "webfetch".into(),
                        "websearch".into(),
                        "lsp".into(),
                    ]),
                    denied_tools: Some(vec![
                        "write".into(),
                        "edit".into(),
                        "shell".into(),
                        "bash".into(),
                        "apply_patch".into(),
                        "todowrite".into(),
                        "todo".into(),
                        "swarm".into(),
                    ]),
                    allow_file_writes: false,
                    allow_network: true,
                    allow_shell: false,
                    allowed_paths: None,
                    rules: Default::default(),
                },
                Self::system_prompt_for(subagent_type),
            ),
            _ => {
                // general: full tools except todo / nested swarm (OpenCode default + safety)
                let mut perm = self.info.permission.clone();
                let mut denied = perm.denied_tools.unwrap_or_default();
                for t in ["todowrite", "todo", "todoread", "swarm"] {
                    if !denied.iter().any(|x| x == t) {
                        denied.push(t.to_string());
                    }
                }
                perm.denied_tools = Some(denied);
                (perm, Self::system_prompt_for("general"))
            }
        };

        let mut info = self.info.clone();
        info.name = subagent_type.to_string();
        info.mode = AgentMode::Subagent;
        info.permission = permission;
        info.system_prompt = Some(Self::with_agents_md(&system_prompt, &session.project_path));

        let task = SubagentTask {
            goal: goal.clone(),
            context,
            tools: None,
            max_turns,
        };

        let runner = SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            info,
            session.project_path.clone(),
            self.sandbox.clone(),
            self.network.clone(),
        )
        .with_memory(self.memory.clone())
        .with_file_index(self.file_index.clone())
        .with_panel(self.panel_sink())
        .with_question_prompter(Arc::clone(&self.question_prompter))
        .with_approval_mode(self.approval_mode);

        let child_id = format!("task-{}", call.id);
        let started = std::time::Instant::now();
        emit(
            &events.cloned(),
            TurnEvent::Subagent {
                id: child_id.clone(),
                kind: subagent_type.to_string(),
                description: goal.clone(),
                status: "running".into(),
                activity: "Thinking".into(),
                elapsed_ms: 0,
                output: String::new(),
            },
        );

        let (worker_provider, worker_model) =
            crate::routing::resolve_worker_model(provider_name, model, self.model_smol.as_deref());
        match runner
            .run(task, &worker_provider, &worker_model, api_key)
            .await
        {
            Ok(result) => {
                if !result.usage.is_empty()
                    && let Ok(mut pending) = self.subagent_usage_pending.lock()
                {
                    pending.add(&result.usage);
                }
                let usage_note = if result.usage.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\n[subagent usage: {} in / {} out]",
                        result.usage.input_tokens, result.usage.output_tokens
                    )
                };
                let status = if result.success {
                    "completed"
                } else {
                    "failed"
                };
                emit(
                    &events.cloned(),
                    TurnEvent::Subagent {
                        id: child_id.clone(),
                        kind: subagent_type.to_string(),
                        description: goal.clone(),
                        status: status.into(),
                        activity: String::new(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        output: result.output.clone(),
                    },
                );
                if result.success {
                    persist_agent_artifact(&session.project_path, &child_id, &result.output);
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: if result.success {
                        format!(
                            "Subagent ({}) completed in {:.1}s. Re-read with `read agent://{child_id}`.\n\n{}{usage_note}",
                            subagent_type,
                            result.duration.as_secs_f64(),
                            result.output
                        )
                    } else {
                        format!(
                            "Subagent ({}) finished with errors:\n\n{}{usage_note}",
                            subagent_type, result.output
                        )
                    },
                    is_error: !result.success,
                }
            }
            Err(e) => {
                emit(
                    &events.cloned(),
                    TurnEvent::Subagent {
                        id: child_id,
                        kind: subagent_type.to_string(),
                        description: goal.clone(),
                        status: "failed".into(),
                        activity: String::new(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        output: e.to_string(),
                    },
                );
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!("Failed to run subagent: {}", e),
                    is_error: true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dispatch_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
