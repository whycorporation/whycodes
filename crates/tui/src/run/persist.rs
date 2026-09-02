//! Session persistence, load, and status reports for the TUI event loop.
use super::*;

/// Best-effort session flush (success, error, or cancel) + structured log.
pub(super) fn persist_session_best_effort(session: &Session, reason: &str) {
    let outcome = with_session_db(|db| session.save_to_db(db));
    match outcome {
        Some(Ok(())) => {
            whycodes_core::logging::emit_sid(
                "session",
                "info",
                "session.persist",
                Some(session.id.as_str()),
                Some(serde_json::json!({
                    "reason": reason,
                    "messages": session.messages.len(),
                })),
            );
        }
        Some(Err(e)) => {
            whycodes_core::logging::emit_sid(
                "session",
                "warn",
                "session.persist_failed",
                Some(session.id.as_str()),
                Some(serde_json::json!({
                    "reason": reason,
                    "error": e.to_string(),
                })),
            );
            tracing::warn!(error = %e, reason, "failed to persist session");
        }
        None => {
            tracing::debug!(reason, "no database available for session persist");
        }
    }
}

/// Process-lifetime SQLite handle for the TUI (avoids re-running migrations
/// and reopening the file on every turn persist).
pub(super) fn with_session_db<T>(
    f: impl FnOnce(&whycodes_storage::db::Database) -> T,
) -> Option<T> {
    use std::sync::{Mutex, OnceLock};
    static DB: OnceLock<Mutex<Option<whycodes_storage::db::Database>>> = OnceLock::new();
    let lock = DB.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().ok()?;
    if guard.is_none() {
        *guard = open_db_quiet();
    }
    guard.as_ref().map(f)
}

pub(super) fn open_db_quiet() -> Option<whycodes_storage::db::Database> {
    let data_dir = whycodes_config::Config::data_dir().ok()?;
    std::fs::create_dir_all(&data_dir).ok()?;
    let db_path = data_dir.join("whycodes.db");
    whycodes_storage::db::Database::open(&db_path.to_string_lossy()).ok()
}

pub(super) fn share_server_up(port: u16) -> bool {
    // Sync quick check via TCP connect (no reqwest dependency in tui path).
    // `SocketAddr::from` is infallible for a `u16` port — no parse/unwrap.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok()
}

pub(super) fn unshare_session(project_dir: &std::path::Path, id: &str) -> usize {
    let mut n = 0usize;
    let candidates = [
        whycodes_core::project_dir(project_dir).join("shares"),
        whycodes_config::Config::data_dir()
            .map(|d| d.join("shares"))
            .unwrap_or_default(),
    ];
    for dir in candidates {
        for ext in ["json", "md"] {
            let p = dir.join(format!("{id}.{ext}"));
            if p.exists() && std::fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Every provider/model pair the config knows about, for the model picker.
pub(super) fn configured_models(config: &Config) -> Vec<(String, String)> {
    crate::app::catalog_models(config)
}

pub(super) fn parse_session_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Stored sessions, newest first, for the session picker.
///
/// A database that will not open is not worth interrupting the user for here —
/// the picker shows its empty state, and `whycodes session list` reports the
/// actual error.
///
/// While building the list, backfill placeholder titles (`New session - …`,
/// `project-ab`) from the first user message so the picker stays useful for
/// sessions created before auto-title or never refined.
pub(super) fn load_session_entries() -> Vec<crate::app::SessionEntry> {
    let Some(db) = with_session_db(|d| {
        // Clone rows we need while the lock is held; do backfill with a second
        // borrow after we drop the map borrow (same connection).
        let rows = d.list_sessions().unwrap_or_default();
        let counts = d.message_counts_by_session().unwrap_or_default();
        (rows, counts)
    }) else {
        return Vec::new();
    };
    let (rows, counts) = db;
    let mut out = Vec::with_capacity(rows.len());
    for s in rows {
        let messages = counts.get(&s.id).copied().unwrap_or(0);
        let mut title = s.title;
        if messages > 0
            && whycodes_session::title::looks_like_default_title(
                &title,
                std::path::Path::new(&s.project_path),
            )
        {
            // Backfill under the shared handle so we do not re-open the DB.
            let upgraded = with_session_db(|d| {
                if let Ok(Some(mut loaded)) = Session::load_from_db(d, &s.id)
                    && loaded.maybe_upgrade_title_from_history()
                {
                    if let Err(err) = loaded.save_to_db(d) {
                        tracing::warn!(error = %err, "failed to persist backfilled session title");
                    }
                    Some(loaded.title)
                } else {
                    None
                }
            })
            .flatten();
            if let Some(t) = upgraded {
                title = t;
            }
        }
        out.push(crate::app::SessionEntry {
            messages,
            id: s.id,
            title,
            updated_at: parse_session_rfc3339(&s.updated_at),
            live: None,
        });
    }
    out
}

pub(super) fn memory_settings(config: &Config) -> whycodes_memory::MemorySettings {
    memory_settings_for(config, None)
}

pub(super) fn memory_settings_for(
    config: &Config,
    agent_bank: Option<String>,
) -> whycodes_memory::MemorySettings {
    let mut s = whycodes_agent::memory_settings_from_config(config);
    s.agent_bank = agent_bank;
    s
}

/// Best-effort code index when the TUI session starts (skips if already indexed).
/// Empty projects return `Some(0)` — do not toast or dirty the idle home.
pub(super) fn maybe_session_auto_index(
    project_dir: &std::path::Path,
    config: &Config,
    app: &mut TuiApp,
) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(n) =
        whycodes_memory::maybe_auto_index(project_dir, &data_dir, &memory_settings(config))
        && n > 0
    {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            format!("Indexed {n} code chunks"),
        );
    }
}

pub(super) fn with_project_memory(
    system_prompt: &str,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) -> String {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    whycodes_memory::apply_memory_prompt(
        system_prompt,
        project_dir,
        &data_dir,
        &memory_settings(config),
        query,
    )
}

pub(super) fn refresh_session_memory(
    session: &mut Session,
    agent: &Agent,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) {
    let base = Agent::with_agents_md(&agent.system_prompt(), project_dir);
    session.set_system_prompt(&with_project_memory(&base, project_dir, config, query));
}

pub(super) fn memory_service(
    project_dir: &std::path::Path,
    config: &Config,
) -> anyhow::Result<whycodes_memory::MemoryService> {
    let data_dir = Config::data_dir()?;
    Ok(whycodes_memory::MemoryService::open(
        project_dir,
        data_dir,
        memory_settings(config),
    )?)
}

/// Shorten a UUID-style session id for status lines (`a1b2c3d4…`).
pub(super) fn short_session_id(id: &str) -> String {
    let take = id.chars().take(8).collect::<String>();
    if id.chars().count() > 8 {
        format!("{take}…")
    } else {
        take
    }
}

/// Load a session by exact id, unique prefix, or [`RESUME_LATEST`].
pub(super) fn try_load_session(want: &str) -> anyhow::Result<Option<Session>> {
    match with_session_db(|db| resolve_and_load_session(db, want)) {
        Some(r) => r,
        None => anyhow::bail!("database unavailable"),
    }
}

/// Resolve `want` against the session table and load the full transcript.
///
/// - [`RESUME_LATEST`] → first row of `list_sessions` (ORDER BY updated_at DESC)
/// - exact id match
/// - otherwise unique prefix (case-insensitive); ambiguous prefix → error
pub fn resolve_and_load_session(
    db: &whycodes_storage::db::Database,
    want: &str,
) -> anyhow::Result<Option<Session>> {
    if want == RESUME_LATEST || want.eq_ignore_ascii_case("latest") {
        let list = db.list_sessions()?;
        let Some(row) = list.into_iter().next() else {
            return Ok(None);
        };
        return Ok(Session::load_from_db(db, &row.id)?);
    }

    if let Some(s) = Session::load_from_db(db, want)? {
        return Ok(Some(s));
    }

    // Prefix match (handy for typing the first 8 chars from `/sessions`).
    let want_l = want.to_ascii_lowercase();
    let list = db.list_sessions()?;
    let matches: Vec<_> = list
        .into_iter()
        .filter(|s| s.id.to_ascii_lowercase().starts_with(&want_l))
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Session::load_from_db(db, &matches[0].id)?),
        n => anyhow::bail!("ambiguous session id prefix '{want}' ({n} matches); use a longer id"),
    }
}

/// Session details for `/info`.
///
/// Reports the provider's own token counts when it gave any. The character
/// heuristic is shown only when it did not, and labelled as an estimate — the
/// two are not the same measurement and presenting them identically would
/// suggest they are.
/// Context window breakdown (`/context` — Claude Code spirit).
pub(super) fn context_report(
    session: &Session,
    app: &TuiApp,
    config: &Config,
    agent: &whycodes_agent::Agent,
) -> String {
    use whycodes_core::types::{MessageContent, Role};

    let mut lines = vec!["Context".to_string()];
    lines.push(format!(
        "  budget:    {} / {} ({}%)",
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent()
    ));
    lines.push(format!(
        "  estimate:  ~{} tok (char heuristic)",
        session.token_count()
    ));
    lines.push(format!(
        "  compact:   threshold={} llm={}",
        config.session.compaction_threshold, config.session.compaction_llm
    ));

    let mut by_role = std::collections::BTreeMap::<&str, usize>::new();
    let mut tool_sizes: Vec<(usize, String)> = Vec::new();
    for (i, m) in session.messages.iter().enumerate() {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        *by_role.entry(role).or_default() += 1;
        if m.role == Role::Tool {
            let chars = match &m.content {
                MessageContent::Text(t) => t.chars().count(),
                MessageContent::Blocks(b) => b
                    .iter()
                    .map(|bl| match bl {
                        whycodes_core::types::ContentBlock::Text { text }
                        | whycodes_core::types::ContentBlock::ToolResult {
                            content: text, ..
                        } => text.chars().count(),
                        _ => 0,
                    })
                    .sum(),
            };
            let label = m.name.clone().unwrap_or_else(|| format!("tool#{i}"));
            tool_sizes.push((chars, label));
        }
    }
    lines.push(format!("  messages:  {}", session.messages.len()));
    for (role, n) in by_role {
        lines.push(format!("    {role}: {n}"));
    }
    tool_sizes.sort_by_key(|e| std::cmp::Reverse(e.0));
    if !tool_sizes.is_empty() {
        lines.push("  largest tool results:".into());
        for (chars, label) in tool_sizes.into_iter().take(8) {
            lines.push(format!("    {label}: {chars} chars"));
        }
    }

    let profile = whycodes_tools::ToolProfile::parse(&config.session.tool_profile);
    let activated = agent.activated_tools_snapshot();
    lines.push(format!(
        "  tools:     profile={} activated={}",
        profile.as_str(),
        if activated.is_empty() {
            "(none)".into()
        } else {
            activated.join(",")
        }
    ));
    lines.push(format!("  memory:    enabled={}", config.memory.enabled));
    if let Some(cwd) = agent.cwd_override_path() {
        lines.push(format!("  cwd:       override {}", cwd.display()));
    } else {
        lines.push(format!("  cwd:       {}", session.project_path.display()));
    }
    lines.join("\n")
}

/// Git status + short diff for the project (Claude Code `/diff` spirit).
pub(super) fn project_diff_report(project_dir: &std::path::Path) -> String {
    let mut out = String::from("Diff\n");
    let status = std::process::Command::new("git")
        .args(["status", "--short", "--branch"])
        .current_dir(project_dir)
        .output();
    match status {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            if s.trim().is_empty() {
                out.push_str("  (clean working tree)\n");
            } else {
                out.push_str("  status:\n");
                for line in s.lines().take(80) {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            out.push_str(&format!(
                "  git status failed: {}\n",
                err.trim().lines().next().unwrap_or("unknown")
            ));
            return out;
        }
        Err(e) => {
            out.push_str(&format!("  git unavailable: {e}\n"));
            return out;
        }
    }

    let diff = std::process::Command::new("git")
        .args(["diff", "--stat", "HEAD"])
        .current_dir(project_dir)
        .output();
    if let Ok(o) = diff
        && o.status.success()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if !s.trim().is_empty() {
            out.push_str("  unstaged/staged vs HEAD:\n");
            for line in s.lines().take(60) {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }

    let staged = std::process::Command::new("git")
        .args(["diff", "--stat", "--cached"])
        .current_dir(project_dir)
        .output();
    if let Ok(o) = staged
        && o.status.success()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if !s.trim().is_empty() {
            out.push_str("  staged only:\n");
            for line in s.lines().take(40) {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }

    out
}

/// Session + last-turn token usage (Claude Code `/cost` spirit).
pub(super) fn cost_report(session: &Session, app: &TuiApp) -> String {
    let mut lines = vec!["Cost / usage".to_string()];
    let u = &session.usage;
    if u.is_empty() {
        lines.push(format!(
            "  session:   ~{} tokens (estimated; provider has not reported usage yet)",
            session.token_count()
        ));
    } else {
        lines.push(format!(
            "  session:   {} in / {} out · total {}",
            format_token_count(u.input_tokens),
            format_token_count(u.output_tokens),
            format_token_count(u.total())
        ));
        if let Some(c) = u.cache_creation_input_tokens.filter(|n| *n > 0) {
            lines.push(format!("  cache write: {}", format_token_count(c)));
        }
        if let Some(r) = u.cache_read_input_tokens.filter(|n| *n > 0) {
            lines.push(format!("  cache read:  {}", format_token_count(r)));
        }
    }
    if let Some(ref turn) = app.turn_usage {
        lines.push(format!(
            "  last turn: {} in / {} out · total {}",
            format_token_count(turn.input_tokens),
            format_token_count(turn.output_tokens),
            format_token_count(turn.total())
        ));
    } else {
        lines.push("  last turn: (none yet)".into());
    }
    lines.push(format!(
        "  context:   {} / {} ({}%)",
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent()
    ));
    lines
        .push("  note:      providers bill differently; figures are token counts, not USD.".into());
    lines.join("\n")
}

/// Claude Code–style environment check: keys, sandbox, git, tools, jobs.
pub(super) fn doctor_report(
    session: &Session,
    app: &TuiApp,
    config: &Config,
    agent: &whycodes_agent::Agent,
    project_dir: &std::path::Path,
) -> String {
    use std::path::Path;

    let mut lines = vec!["Doctor".to_string()];

    // ── Provider / model ──────────────────────────────────────────────
    let provider = app.provider_name.as_str();
    let model = app.model_name.as_str();
    lines.push(format!("  provider:     {provider}"));
    lines.push(format!("  model:        {model}"));
    lines.push(format!("  agent:        {}", agent.info.name));
    lines.push(format!("  tool_profile: {}", config.session.tool_profile));

    // API key present? (never print the key)
    let env_name = format!("{}_API_KEY", provider.to_uppercase());
    let key_ok = config
        .providers
        .get(provider)
        .and_then(|pc| pc.api_key.as_ref())
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
        || std::env::var(&env_name)
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false);
    lines.push(format!(
        "  api_key:      {}",
        if key_ok {
            "set"
        } else {
            "MISSING — /connect or env"
        }
    ));

    // ── Paths ─────────────────────────────────────────────────────────
    lines.push(format!("  project:      {}", project_dir.display()));
    lines.push(format!("  session_id:   {}", session.id));

    let git = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    lines.push(format!(
        "  git_repo:     {}",
        if git { "yes" } else { "no" }
    ));

    // ── Safety ────────────────────────────────────────────────────────
    lines.push(format!(
        "  bash_risk:    {}",
        config.security.bash_risk_threshold
    ));
    lines.push(format!(
        "  sandbox:      mode={} network={}",
        config.security.sandbox, config.security.sandbox_network
    ));
    #[cfg(target_os = "linux")]
    {
        let bwrap = Path::new("/usr/bin/bwrap").is_file() || which_bwrap();
        lines.push(format!(
            "  bwrap:        {}",
            if bwrap {
                "available"
            } else {
                "not found (host fallback)"
            }
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        lines.push("  bwrap:        n/a (non-Linux)".into());
    }

    // ── Automation ────────────────────────────────────────────────────
    let bg = agent.background_registry();
    let running = bg.running_count();
    let total = bg.list().len();
    lines.push(format!(
        "  background:   {running} running / {total} known (max {})",
        config.automation.max_background_jobs
    ));
    lines.push(format!(
        "  swarm:        enabled={} worktrees={}",
        config.swarm.enabled, config.swarm.worktrees
    ));
    lines.push(format!(
        "  compaction:   threshold={}",
        config.session.compaction_threshold
    ));
    lines.push(format!(
        "  context:      {} / {} ({}%)",
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent()
    ));

    // ── Quick health summary ──────────────────────────────────────────
    let mut issues = Vec::new();
    if !key_ok {
        issues.push("API key missing for active provider");
    }
    if !project_dir.is_dir() {
        issues.push("project directory missing");
    }
    if issues.is_empty() {
        lines.push("  status:       ok".into());
    } else {
        lines.push(format!("  status:       issues — {}", issues.join("; ")));
    }

    lines.join("\n")
}

#[cfg(target_os = "linux")]
pub(super) fn which_bwrap() -> bool {
    std::process::Command::new("which")
        .arg("bwrap")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(super) fn session_details(
    session: &Session,
    agent: &str,
    app: &TuiApp,
    config: &Config,
) -> String {
    let usage = &session.usage;
    let profile = whycodes_tools::ToolProfile::parse(&config.session.tool_profile);
    let mut out = format!(
        "Session\n  title:     {}\n  source:    {:?}\n  id:        {}\n  agent:     {agent}\n  messages:  {}\n  model:     {}/{}\n  context:   {} / {} ({}%)\n  tools:     profile={}\n  prompt_cache: {}\n",
        session.title,
        session.title_source,
        session.id,
        session.messages.len(),
        app.provider_name,
        app.model_name,
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent(),
        profile.as_str(),
        config.session.prompt_cache,
    );
    if let Some(ref fast) = config.session.model_fast {
        out.push_str(&format!("  model_fast: {fast}\n"));
    } else {
        out.push_str("  model_fast: (auto small sibling on trivial chat)\n");
    }
    if let Some(ref smol) = config.session.model_smol {
        out.push_str(&format!("  model_smol: {smol}\n"));
    } else {
        out.push_str("  model_smol: (auto small sibling for task/swarm)\n");
    }
    if let Some(ref plan) = config.session.model_plan {
        out.push_str(&format!("  model_plan: {plan}\n"));
    }
    if !config.session.stream_rules.is_empty() {
        out.push_str(&format!(
            "  stream_rules: {}\n",
            config.session.stream_rules.len()
        ));
    }
    out.push_str(&format!(
        "  model_race: {} (after {}ms)\n  response_cache: {}\n",
        config.session.model_race, config.session.race_after_ms, config.session.response_cache
    ));
    out.push_str(&format!(
        "  swarm:     enabled={} max_agents={} worktrees={}\n",
        config.swarm.enabled, config.swarm.max_agents, config.swarm.worktrees
    ));

    if usage.is_empty() {
        out.push_str(&format!(
            "  tokens:    ~{} (estimated; the provider has not reported usage yet)\n",
            session.token_count()
        ));
        return out;
    }

    out.push_str(&format!(
        "  input:     {}\n  output:    {}\n",
        usage.input_tokens, usage.output_tokens
    ));
    if let Some(created) = usage.cache_creation_input_tokens {
        out.push_str(&format!("  cache write: {created}\n"));
    }
    if let Some(read) = usage.cache_read_input_tokens {
        out.push_str(&format!("  cache read:  {read}\n"));
    }
    out.push_str(&format!("  total:     {}\n", usage.total()));
    out
}
