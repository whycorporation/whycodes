//! Grok-style logging: always-on JSONL, optional file log, crash reports.
//!
//! Layout under [`Config::data_dir`]:
//! ```text
//! <data_dir>/
//!   logs/unified.jsonl    # structured events (append-only)
//!   crash/crash-*.txt     # panic reports
//!   debug/whycode-*.log   # --debug / WHYCODE_LOG_FILE human logs
//!   debug/latest.log      # symlink (or copy) of the active debug log
//! ```
//!
//! Environment:
//! - `RUST_LOG` / `WHYCODE_LOG_LEVEL` — filter (e.g. `debug`, `info,whycode_agent=debug`)
//! - `WHYCODE_LOG_FILE` — extra human-readable log path (verbatim)
//! - `RUST_BACKTRACE` — included in crash reports when set

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

/// Process-wide logging state after a successful [`init`].
static STATE: OnceLock<LoggingState> = OnceLock::new();

/// Optional terminal / TUI cleanup run from the panic hook (raw mode, alt screen).
pub(crate) static PANIC_CLEANUP: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

/// Whether [`init`] has completed successfully.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Package version embedded in every JSONL line.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Paths ──────────────────────────────────────────────────────────────────

/// Standard log / crash / debug directories under the data dir.
#[derive(Debug, Clone)]
pub struct LogDirs {
    pub root: PathBuf,
    pub logs: PathBuf,
    pub crash: PathBuf,
    pub debug: PathBuf,
}

impl LogDirs {
    pub fn from_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        let root = data_dir.into();
        Self {
            logs: root.join("logs"),
            crash: root.join("crash"),
            debug: root.join("debug"),
            root,
        }
    }

    /// Create `logs/`, `crash/`, and `debug/` if missing.
    pub fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(&self.logs)?;
        fs::create_dir_all(&self.crash)?;
        fs::create_dir_all(&self.debug)?;
        Ok(())
    }

    pub fn unified_jsonl(&self) -> PathBuf {
        self.logs.join("unified.jsonl")
    }

    pub fn crash_report_path(&self, stamp: &str) -> PathBuf {
        self.crash.join(format!("crash-{stamp}.txt"))
    }
}

// ── JSONL event ────────────────────────────────────────────────────────────

/// One line of `logs/unified.jsonl` (Grok-compatible shape).
#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    pub ts: String,
    pub src: String,
    pub pid: u32,
    pub ver: String,
    pub lvl: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<serde_json::Value>,
}

impl LogEvent {
    pub fn new(src: impl Into<String>, lvl: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            src: src.into(),
            pid: std::process::id(),
            ver: VERSION.to_string(),
            lvl: lvl.into(),
            sid: None,
            msg: msg.into(),
            ctx: None,
        }
    }

    pub fn with_sid(mut self, sid: impl Into<String>) -> Self {
        self.sid = Some(sid.into());
        self
    }

    pub fn with_ctx(mut self, ctx: serde_json::Value) -> Self {
        self.ctx = Some(ctx);
        self
    }
}

/// Append a single JSON line to `path` (creates parent dirs as needed).
pub fn append_jsonl(path: &Path, event: &LogEvent) -> io::Result<()> {
    ensure_parent(path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, event).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

pub(crate) fn ensure_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

/// Emit a structured event to the process-wide `unified.jsonl` (no-op if not init'd).
pub fn emit(src: &str, lvl: &str, msg: &str, ctx: Option<serde_json::Value>) {
    if let Some(state) = STATE.get() {
        let mut event = LogEvent::new(src, lvl, msg);
        event.pid = state.pid;
        event.ver = state.version.clone();
        event.ctx = ctx;
        let _ = write_jsonl_locked(&state.unified, &event);
    }
}

/// Like [`emit`] but attaches a session id.
pub fn emit_sid(
    src: &str,
    lvl: &str,
    msg: &str,
    sid: Option<&str>,
    ctx: Option<serde_json::Value>,
) {
    if let Some(state) = STATE.get() {
        let mut event = LogEvent::new(src, lvl, msg);
        event.pid = state.pid;
        event.ver = state.version.clone();
        event.sid = sid.map(|s| s.to_string());
        event.ctx = ctx;
        let _ = write_jsonl_locked(&state.unified, &event);
    }
}

pub(crate) fn write_jsonl_locked(file: &Mutex<File>, event: &LogEvent) -> io::Result<()> {
    let mut guard = lock_log_file(file)?;
    serde_json::to_writer(&mut *guard, event).map_err(io::Error::other)?;
    guard.write_all(b"\n")?;
    guard.flush()?;
    Ok(())
}

pub(crate) fn lock_log_file(file: &Mutex<File>) -> io::Result<std::sync::MutexGuard<'_, File>> {
    file.lock()
        .map_err(|e| io::Error::other(format!("log lock poisoned: {e}")))
}

// ── Init ───────────────────────────────────────────────────────────────────

/// Options for [`init`].
#[derive(Debug, Clone)]
pub struct InitOptions {
    /// Data directory (`Config::data_dir()`).
    pub data_dir: PathBuf,
    /// Level / filter string (`info`, `debug`, module filters).
    pub log_level: Option<String>,
    /// Optional human log file (`WHYCODE_LOG_FILE`).
    pub log_file: Option<PathBuf>,
    /// When true, also write a debug log under `data_dir/debug/`.
    pub debug: bool,
    /// Mirror human logs to stderr (disable in full-screen TUI).
    pub with_stderr: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("."),
            log_level: None,
            log_file: None,
            debug: false,
            with_stderr: true,
        }
    }
}

/// Handle returned by [`init`] (cloneable path info).
#[derive(Debug, Clone)]
pub struct LoggingHandle {
    pub dirs: LogDirs,
    /// Human-readable debug/log file path if one was opened.
    pub debug_log: Option<PathBuf>,
}

struct LoggingState {
    dirs: LogDirs,
    unified: Mutex<File>,
    pid: u32,
    version: String,
}

/// Initialize process logging. Safe to call once; subsequent calls return the
/// existing handle without re-installing the subscriber.
pub fn init(opts: InitOptions) -> crate::Result<LoggingHandle> {
    if let Some(state) = STATE.get() {
        return Ok(LoggingHandle {
            dirs: state.dirs.clone(),
            debug_log: None,
        });
    }

    let dirs = LogDirs::from_data_dir(&opts.data_dir);
    dirs.ensure().map_err(crate::Error::Io)?;

    let unified_path = dirs.unified_jsonl();
    let unified_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&unified_path)
        .map_err(crate::Error::Io)?;

    let pid = std::process::id();
    let state = LoggingState {
        dirs: dirs.clone(),
        unified: Mutex::new(unified_file),
        pid,
        version: VERSION.to_string(),
    };

    // Human-readable log path: explicit env wins, else --debug creates one.
    let debug_log = resolve_debug_log_path(&dirs, &opts)?;

    let filter = build_env_filter(opts.log_level.as_deref());

    // Build subscriber layers.
    let jsonl_layer = JsonlLayer {
        file: Arc::new(Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&unified_path)
                .map_err(crate::Error::Io)?,
        )),
        pid,
        version: VERSION.to_string(),
    };

    let registry = Registry::default().with(filter).with(jsonl_layer);

    // Optional file + stderr fmt layers.
    let file_layer = choose_debug_file(debug_log.as_deref())?.map(fmt_file_layer);

    let stderr_layer = maybe_layer(opts.with_stderr, stderr_fmt_layer());

    // try_init so tests / double-main don't panic
    let result = registry.with(file_layer).with(stderr_layer).try_init();
    note_try_init(result.is_ok());

    let _ = STATE.set(state);
    INITIALIZED.store(true, Ordering::SeqCst);

    install_panic_hook_inner(dirs.clone());

    emit(
        "whycode",
        "info",
        "logging.init",
        Some(json!({
            "unified": unified_path.display().to_string(),
            "debug_log": debug_log.as_ref().map(|p| p.display().to_string()),
            "with_stderr": opts.with_stderr,
            "debug": opts.debug,
        })),
    );

    Ok(LoggingHandle { dirs, debug_log })
}

pub(crate) fn resolve_debug_log_path(
    dirs: &LogDirs,
    opts: &InitOptions,
) -> crate::Result<Option<PathBuf>> {
    if let Some(ref p) = opts.log_file {
        create_parent_dir(p)?;
        link_latest(dirs, p);
        return Ok(Some(p.clone()));
    }
    if opts.debug {
        let stamp = Utc::now().format("%Y%m%dT%H%M%S");
        let path = dirs
            .debug
            .join(format!("whycode-{}-{}.log", std::process::id(), stamp));
        // Touch file so latest link has a target.
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(crate::Error::Io)?;
        link_latest(dirs, &path);
        return Ok(Some(path));
    }
    Ok(None)
}

fn link_latest(dirs: &LogDirs, target: &Path) {
    let latest = dirs.debug.join("latest.log");
    let _ = fs::remove_file(&latest);
    #[cfg(unix)]
    {
        symlink_or_copy(target, &latest);
    }
    #[cfg(not(unix))]
    {
        let _ = fs::write(dirs.debug.join("latest.path"), target.display().to_string());
    }
}

pub(crate) fn open_append(path: &Path) -> crate::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(crate::Error::Io)
}

pub(crate) fn create_parent_dir(p: &Path) -> crate::Result<()> {
    if let Some(parent) = p.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(crate::Error::Io)?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn symlink_or_copy(target: &Path, latest: &Path) {
    if std::os::unix::fs::symlink(target, latest).is_err() {
        let _ = fs::copy(target, latest);
    }
}

pub(crate) fn fmt_file_layer<S>(
    file: File,
) -> tracing_subscriber::fmt::Layer<
    S,
    tracing_subscriber::fmt::format::DefaultFields,
    tracing_subscriber::fmt::format::Format,
    Mutex<File>,
>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .with_span_events(FmtSpan::NONE)
        .with_target(true)
}

pub(crate) fn choose_debug_file(path: Option<&Path>) -> crate::Result<Option<File>> {
    match path {
        Some(p) => Ok(Some(open_append(p)?)),
        None => Ok(None),
    }
}

pub(crate) fn emit_panic_report(result: io::Result<PathBuf>, payload: &str) {
    eprintln!("{}", crash_user_message(&result));
    let Ok(path) = result else {
        return;
    };
    emit(
        "whycode",
        "error",
        "panic",
        Some(json!({
            "path": path.display().to_string(),
            "payload": payload,
        })),
    );
}

pub(crate) fn format_location(loc: Option<&std::panic::Location<'_>>) -> Option<String> {
    loc.map(|loc| format!("location: {}:{}:{}\n", loc.file(), loc.line(), loc.column()))
}

pub(crate) fn crash_user_message(result: &io::Result<PathBuf>) -> String {
    match result {
        Ok(path) => format!("\nwhycode crashed — report written to {}", path.display()),
        Err(e) => format!("\nwhycode crashed — failed to write crash report: {e}"),
    }
}

pub(crate) fn clean_debug_value(s: String) -> String {
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .map(|x| x.to_string())
        .unwrap_or(s)
}

pub(crate) fn build_env_filter(explicit: Option<&str>) -> EnvFilter {
    // Priority: RUST_LOG → explicit (config / WHYCODE_LOG_LEVEL) → info
    if let Ok(filter) = EnvFilter::try_from_default_env() {
        return filter;
    }
    if let Some(level) = explicit
        && let Ok(f) = EnvFilter::try_new(level)
    {
        return f;
    }
    if let Ok(level) = std::env::var("WHYCODE_LOG_LEVEL")
        && let Ok(f) = EnvFilter::try_new(&level)
    {
        return f;
    }
    EnvFilter::new("info")
}

pub(crate) fn maybe_layer<T>(on: bool, layer: T) -> Option<T> {
    if on { Some(layer) } else { None }
}

pub(crate) fn note_try_init(ok: bool) {
    warn_already_set((!ok).then(|| "already".into()));
}

pub(crate) fn event_message(message: Option<String>, fallback: &str) -> String {
    match message {
        Some(m) => m,
        None => fallback.to_string(),
    }
}

pub(crate) fn warn_already_set(err: Option<String>) {
    if let Some(e) = err {
        tracing::warn!("tracing subscriber already set: {e}");
    }
}

pub(crate) fn stderr_fmt_layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_span_events(FmtSpan::NONE)
        .with_target(true)
}

/// Whether logging has been initialized.
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::SeqCst)
}

/// Current log directories if init has run.
pub fn dirs() -> Option<LogDirs> {
    STATE.get().map(|s| s.dirs.clone())
}

// ── Panic hook ─────────────────────────────────────────────────────────────

/// Register a cleanup callback (e.g. leave alternate screen) run before crash I/O.
pub fn set_panic_cleanup(f: impl Fn() + Send + Sync + 'static) {
    *recover_cleanup() = Some(Arc::new(f));
}

/// Clear the panic cleanup callback (e.g. after clean TUI exit).
pub fn clear_panic_cleanup() {
    *recover_cleanup() = None;
}

fn recover_cleanup() -> std::sync::MutexGuard<'static, Option<Arc<dyn Fn() + Send + Sync>>> {
    recover_lock(PANIC_CLEANUP.lock())
}

pub(crate) fn recover_lock<T>(res: std::sync::LockResult<T>) -> T {
    match res {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[cfg(test)]
pub(crate) fn poison_panic_cleanup() {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = PANIC_CLEANUP.lock().unwrap();
        panic!("poison");
    }));
}

fn install_panic_hook_inner(dirs: LogDirs) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Restore terminal first so the user can read the panic / shell works.
        if let Ok(slot) = PANIC_CLEANUP.try_lock()
            && let Some(ref cleanup) = *slot
        {
            cleanup();
        }

        emit_panic_report(write_crash_report(&dirs, info), &panic_message(info));

        default_hook(info);
    }));
}

/// Format and write a crash report. Public for tests.
pub fn write_crash_report(
    dirs: &LogDirs,
    info: &std::panic::PanicHookInfo<'_>,
) -> io::Result<PathBuf> {
    dirs.ensure()?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3f");
    let path = dirs.crash_report_path(&stamp.to_string());
    let body = format_crash_report(info);
    fs::write(&path, body)?;
    Ok(path)
}

/// Build crash report text (no I/O). Public for tests.
pub fn format_crash_report(info: &std::panic::PanicHookInfo<'_>) -> String {
    let mut out = String::new();
    out.push_str("whycode crash report\n");
    out.push_str(&format!("version: {VERSION}\n"));
    out.push_str(&format!("pid: {}\n", std::process::id()));
    out.push_str(&format!(
        "time: {}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    ));
    if let Some(line) = format_location(info.location()) {
        out.push_str(&line);
    }
    out.push_str(&format!("message: {}\n", panic_message(info)));
    out.push_str(&format!(
        "RUST_BACKTRACE: {}\n",
        std::env::var("RUST_BACKTRACE").unwrap_or_else(|_| "unset".into())
    ));
    // Capture a backtrace when the runtime provides one via std.
    out.push_str("\nbacktrace:\n");
    out.push_str(&format!(
        "{:?}\n",
        std::backtrace::Backtrace::force_capture()
    ));
    out
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    "Box<dyn Any>".to_string()
}

// ── Tracing → JSONL layer ──────────────────────────────────────────────────

struct JsonlLayer {
    file: Arc<Mutex<File>>,
    pid: u32,
    version: String,
}

impl JsonlLayer {
    pub(crate) fn record(&self, event: &Event<'_>) {
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);

        let meta = event.metadata();
        let lvl = meta.level().as_str().to_ascii_lowercase();
        let src = meta.target().to_string();
        let msg = event_message(visitor.message.take(), meta.name());

        let ctx_map = visitor.fields;
        let log_event = LogEvent {
            ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            src,
            pid: self.pid,
            ver: self.version.clone(),
            lvl,
            sid: visitor.sid,
            msg,
            ctx: if ctx_map.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(ctx_map))
            },
        };

        let _ = write_jsonl_locked(&self.file, &log_event);
    }
}

impl<S> Layer<S> for JsonlLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        self.record(event);
    }
}

#[derive(Default)]
pub(crate) struct JsonVisitor {
    pub(crate) message: Option<String>,
    pub(crate) sid: Option<String>,
    pub(crate) fields: serde_json::Map<String, serde_json::Value>,
}

impl JsonVisitor {
    pub(crate) fn assign_str(&mut self, name: &str, value: String) {
        if name == "message" {
            self.message = Some(value);
        } else if name == "sid" {
            self.sid = Some(value);
        } else {
            self.fields.insert(name.to_string(), json!(value));
        }
    }
}

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.assign_str(field.name(), clean_debug_value(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.assign_str(field.name(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.insert(field.name().to_string(), json!(value));
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────
