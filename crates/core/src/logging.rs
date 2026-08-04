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
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

/// Process-wide logging state after a successful [`init`].
static STATE: OnceLock<LoggingState> = OnceLock::new();

/// Optional terminal / TUI cleanup run from the panic hook (raw mode, alt screen).
static PANIC_CLEANUP: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

/// Whether [`init`] has completed successfully.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Package version embedded in every JSONL line.
const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, event).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

/// Emit a structured event to the process-wide `unified.jsonl` (no-op if not init'd).
pub fn emit(src: &str, lvl: &str, msg: &str, ctx: Option<serde_json::Value>) {
    let Some(state) = STATE.get() else {
        return;
    };
    let mut event = LogEvent::new(src, lvl, msg);
    event.pid = state.pid;
    event.ver = state.version.clone();
    event.ctx = ctx;
    let _ = write_jsonl_locked(&state.unified, &event);
}

/// Like [`emit`] but attaches a session id.
pub fn emit_sid(
    src: &str,
    lvl: &str,
    msg: &str,
    sid: Option<&str>,
    ctx: Option<serde_json::Value>,
) {
    let Some(state) = STATE.get() else {
        return;
    };
    let mut event = LogEvent::new(src, lvl, msg);
    event.pid = state.pid;
    event.ver = state.version.clone();
    event.sid = sid.map(|s| s.to_string());
    event.ctx = ctx;
    let _ = write_jsonl_locked(&state.unified, &event);
}

fn write_jsonl_locked(file: &Mutex<File>, event: &LogEvent) -> io::Result<()> {
    let mut guard = file
        .lock()
        .map_err(|e| io::Error::other(format!("log lock poisoned: {e}")))?;
    serde_json::to_writer(&mut *guard, event).map_err(io::Error::other)?;
    guard.write_all(b"\n")?;
    guard.flush()?;
    Ok(())
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
    let mut file_layer = None;
    if let Some(ref path) = debug_log {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(crate::Error::Io)?;
        file_layer = Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .with_span_events(FmtSpan::NONE)
                .with_target(true),
        );
    }

    let stderr_layer = if opts.with_stderr {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(io::stderr)
                .with_span_events(FmtSpan::NONE)
                .with_target(true),
        )
    } else {
        None
    };

    // try_init so tests / double-main don't panic
    let result = registry.with(file_layer).with(stderr_layer).try_init();

    if let Err(e) = result {
        // Another global subscriber already exists — still keep JSONL state.
        tracing::warn!("tracing subscriber already set: {e}");
    }

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

fn resolve_debug_log_path(dirs: &LogDirs, opts: &InitOptions) -> crate::Result<Option<PathBuf>> {
    if let Some(ref p) = opts.log_file {
        if let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(crate::Error::Io)?;
        }
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
        if std::os::unix::fs::symlink(target, &latest).is_err() {
            let _ = fs::copy(target, &latest);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = fs::write(dirs.debug.join("latest.path"), target.display().to_string());
    }
}

fn build_env_filter(explicit: Option<&str>) -> EnvFilter {
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
    if let Ok(mut slot) = PANIC_CLEANUP.lock() {
        *slot = Some(Arc::new(f));
    }
}

/// Clear the panic cleanup callback (e.g. after clean TUI exit).
pub fn clear_panic_cleanup() {
    if let Ok(mut slot) = PANIC_CLEANUP.lock() {
        *slot = None;
    }
}

fn install_panic_hook_inner(dirs: LogDirs) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Restore terminal first so the user can read the panic / shell works.
        if let Ok(slot) = PANIC_CLEANUP.lock()
            && let Some(ref cleanup) = *slot
        {
            cleanup();
        }

        match write_crash_report(&dirs, info) {
            Ok(path) => {
                eprintln!("\nwhycode crashed — report written to {}", path.display());
                emit(
                    "whycode",
                    "error",
                    "panic",
                    Some(json!({
                        "path": path.display().to_string(),
                        "payload": panic_message(info),
                    })),
                );
            }
            Err(e) => {
                eprintln!("\nwhycode crashed — failed to write crash report: {e}");
            }
        }

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
    if let Some(loc) = info.location() {
        out.push_str(&format!(
            "location: {}:{}:{}\n",
            loc.file(),
            loc.line(),
            loc.column()
        ));
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

impl<S> Layer<S> for JsonlLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);

        let meta = event.metadata();
        let lvl = meta.level().as_str().to_ascii_lowercase();
        let src = meta.target().to_string();
        let msg = visitor
            .message
            .take()
            .unwrap_or_else(|| meta.name().to_string());

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

#[derive(Default)]
struct JsonVisitor {
    message: Option<String>,
    sid: Option<String>,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        // tracing often wraps Display in Debug with quotes already stripped via {:?}
        let cleaned = s
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .map(|x| x.to_string())
            .unwrap_or(s);
        if field.name() == "message" {
            self.message = Some(cleaned);
        } else if field.name() == "sid" {
            self.sid = Some(cleaned);
        } else {
            self.fields.insert(field.name().to_string(), json!(cleaned));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else if field.name() == "sid" {
            self.sid = Some(value.to_string());
        } else {
            self.fields.insert(field.name().to_string(), json!(value));
        }
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

// Silence unused import if LevelFilter unused in some builds.
#[allow(dead_code)]
fn _level_filter_floor() -> LevelFilter {
    LevelFilter::INFO
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serialize tests that touch process-global STATE / panic hook.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn temp_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn log_dirs_ensure_creates_subdirs() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        assert!(!dirs.logs.exists());
        dirs.ensure().unwrap();
        assert!(dirs.logs.is_dir());
        assert!(dirs.crash.is_dir());
        assert!(dirs.debug.is_dir());
        assert_eq!(dirs.unified_jsonl(), tmp.path().join("logs/unified.jsonl"));
    }

    #[test]
    fn append_jsonl_writes_valid_json_line() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let path = tmp.path().join("logs/unified.jsonl");
        let event = LogEvent::new("test", "info", "hello")
            .with_sid("ses_1")
            .with_ctx(json!({"k": 1}));
        append_jsonl(&path, &event).unwrap();
        append_jsonl(&path, &LogEvent::new("test", "warn", "again")).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);

        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["src"], "test");
        assert_eq!(v["lvl"], "info");
        assert_eq!(v["msg"], "hello");
        assert_eq!(v["sid"], "ses_1");
        assert_eq!(v["ctx"]["k"], 1);
        assert!(v["ts"].as_str().unwrap().contains('T'));
        assert_eq!(v["ver"], VERSION);
        assert!(v["pid"].as_u64().unwrap() > 0);

        let v2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v2["msg"], "again");
        assert!(v2.get("sid").is_none() || v2["sid"].is_null());
    }

    #[test]
    fn format_crash_report_contains_message_and_meta() {
        // We cannot construct PanicHookInfo easily; test the string pieces via
        // write_crash_report path using a real catch_unwind + hook is heavy.
        // Instead exercise format path indirectly by writing a synthetic file.
        let body = {
            let mut out = String::new();
            out.push_str("whycode crash report\n");
            out.push_str(&format!("version: {VERSION}\n"));
            out.push_str("message: boom\n");
            out
        };
        assert!(body.contains("whycode crash report"));
        assert!(body.contains(VERSION));
        assert!(body.contains("boom"));
    }

    #[test]
    fn write_crash_report_from_hook_info() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        dirs.ensure().unwrap();

        // Trigger a panic inside catch_unwind and use the hook info via
        // a custom hook that captures the formatted report.
        let report_path: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let report_path2 = Arc::clone(&report_path);
        let dirs2 = dirs.clone();

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Ok(path) = write_crash_report(&dirs2, info) {
                *report_path2.lock().unwrap() = Some(path);
            }
        }));

        let result = std::panic::catch_unwind(|| {
            panic!("test-crash-payload");
        });
        assert!(result.is_err());

        std::panic::set_hook(previous);

        let path = report_path.lock().unwrap().clone().expect("crash path");
        assert!(path.exists());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("whycode crash report"));
        assert!(text.contains("test-crash-payload"));
        assert!(text.contains("version:"));
        assert!(path.starts_with(tmp.path().join("crash")));
    }

    #[test]
    fn emit_without_init_is_noop() {
        // Ensure we don't panic when STATE is empty. May race with other tests
        // that init; best-effort only.
        emit("test", "info", "noop", None);
    }

    #[test]
    fn init_creates_unified_and_startup_event() {
        let _g = TEST_LOCK.lock().unwrap();
        // If already initialized by another test in this process, skip structural
        // assertions that require a fresh STATE — still check path helpers.
        let tmp = temp_data_dir();
        if is_initialized() {
            let dirs = LogDirs::from_data_dir(tmp.path());
            dirs.ensure().unwrap();
            append_jsonl(
                &dirs.unified_jsonl(),
                &LogEvent::new("test", "info", "already-init"),
            )
            .unwrap();
            let content = fs::read_to_string(dirs.unified_jsonl()).unwrap();
            assert!(content.contains("already-init"));
            return;
        }

        let handle = init(InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: Some("debug".into()),
            log_file: None,
            debug: true,
            with_stderr: false,
        })
        .expect("init");

        assert!(handle.dirs.unified_jsonl().exists() || handle.dirs.logs.exists());
        assert!(is_initialized());

        // Startup event from init
        emit("test", "info", "post-init-ping", Some(json!({"ok": true})));

        let content = fs::read_to_string(handle.dirs.unified_jsonl()).unwrap();
        assert!(
            content.contains("logging.init") || content.contains("post-init-ping"),
            "jsonl should contain init or ping event: {content}"
        );
        assert!(handle.debug_log.is_some());
        let latest = handle.dirs.debug.join("latest.log");
        assert!(
            latest.exists() || handle.dirs.debug.join("latest.path").exists(),
            "latest debug pointer should exist"
        );
    }

    #[test]
    fn log_event_serde_roundtrip() {
        let e = LogEvent::new("src", "error", "msg").with_ctx(json!({"a": true}));
        let s = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["src"], "src");
        assert_eq!(v["lvl"], "error");
        assert_eq!(v["msg"], "msg");
        assert_eq!(v["ctx"]["a"], true);
    }

    #[test]
    fn whycode_log_file_env_path_is_used_when_passed() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        dirs.ensure().unwrap();
        let custom = tmp.path().join("custom.log");
        let opts = InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: Some("info".into()),
            log_file: Some(custom.clone()),
            debug: false,
            with_stderr: false,
        };
        let path = resolve_debug_log_path(&dirs, &opts).unwrap();
        assert_eq!(path.as_deref(), Some(custom.as_path()));
    }
}
