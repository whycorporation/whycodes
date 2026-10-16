//! What `whycodes upgrade` (and the TUI's "Update now?") shows while it works.
//!
//! The download used to run with no output at all: the TUI closed and the
//! shell sat blank until a one-line result appeared. On a terminal this draws
//! one line, redrawn ~12 times a second with a spinner, the stage and the
//! bytes so far. Elsewhere (CI logs, pipes) each stage is printed once.
//!
//! The line is rewritten with `\r` and space padding only, no escape
//! sequences, so a Windows console without VT processing shows it cleanly.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: Duration = Duration::from_millis(80);
/// `Content-Length` unknown.
const NO_TOTAL: u64 = u64::MAX;

type Sink = Box<dyn Write + Send>;

struct Shared {
    stage: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    stop: AtomicBool,
    /// Characters on the progress line now, so the next frame (or the
    /// final clear) can blank what a shorter text would leave behind.
    drawn: AtomicUsize,
    out: Mutex<Sink>,
}

/// Progress for one upgrade run. Drop or [`Progress::finish`] clears the line.
pub(crate) struct Progress {
    shared: Option<Arc<Shared>>,
    animate: bool,
    ticker: Option<JoinHandle<()>>,
}

impl Progress {
    /// Draws on stderr: animated on a terminal, one line per stage otherwise.
    pub(crate) fn stderr() -> Self {
        use std::io::IsTerminal;
        let animate =
            std::io::stderr().is_terminal() && std::env::var("TERM").map_or(true, |t| t != "dumb");
        Self::to(Box::new(std::io::stderr()), animate)
    }

    /// Shows nothing.
    pub(crate) fn silent() -> Self {
        Self {
            shared: None,
            animate: false,
            ticker: None,
        }
    }

    pub(crate) fn to(out: Sink, animate: bool) -> Self {
        let shared = Arc::new(Shared {
            stage: Mutex::new(String::new()),
            done: AtomicU64::new(0),
            total: AtomicU64::new(NO_TOTAL),
            stop: AtomicBool::new(false),
            drawn: AtomicUsize::new(0),
            out: Mutex::new(out),
        });
        let ticker = animate.then(|| {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                let mut frame = 0usize;
                while !shared.stop.load(Ordering::Relaxed) {
                    draw(&shared, frame);
                    frame = frame.wrapping_add(1);
                    std::thread::sleep(TICK);
                }
            })
        });
        Self {
            shared: Some(shared),
            animate,
            ticker,
        }
    }

    /// Start a stage ("Downloading v0.6.11"); resets the byte counter.
    pub(crate) fn stage(&self, label: &str) {
        let Some(shared) = &self.shared else {
            return;
        };
        *lock(&shared.stage) = label.to_string();
        shared.done.store(0, Ordering::Relaxed);
        shared.total.store(NO_TOTAL, Ordering::Relaxed);
        if !self.animate && !shared.stop.load(Ordering::Relaxed) {
            emit(shared, &format!("  {label}…\n"));
        }
    }

    pub(crate) fn set_total(&self, total: Option<u64>) {
        if let Some(shared) = &self.shared {
            shared
                .total
                .store(total.unwrap_or(NO_TOTAL), Ordering::Relaxed);
        }
    }

    pub(crate) fn advance(&self, bytes: u64) {
        if let Some(shared) = &self.shared {
            shared.done.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Stop the spinner and clear its line so the caller's result line
    /// starts on a clean row.
    pub(crate) fn finish(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Some(shared) = &self.shared else {
            return;
        };
        shared.stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.ticker.take() {
            if ticker.join().is_err() {
                tracing::debug!("upgrade progress thread panicked");
            }
            let drawn = shared.drawn.swap(0, Ordering::Relaxed);
            if drawn > 0 {
                emit(shared, &format!("\r{}\r", " ".repeat(drawn)));
            }
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.stop();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn draw(shared: &Shared, frame: usize) {
    let stage = lock(&shared.stage).clone();
    if stage.is_empty() {
        return;
    }
    let total = shared.total.load(Ordering::Relaxed);
    let line = progress_line(
        frame,
        &stage,
        shared.done.load(Ordering::Relaxed),
        (total != NO_TOTAL).then_some(total),
    );
    let width = line.chars().count();
    let pad = shared
        .drawn
        .swap(width, Ordering::Relaxed)
        .saturating_sub(width);
    emit(shared, &format!("\r{line}{}", " ".repeat(pad)));
}

/// Write and flush. Output that stops being accepted (stderr closed) turns
/// the progress off; the upgrade itself carries on.
fn emit(shared: &Shared, text: &str) {
    let mut out = lock(&shared.out);
    if let Err(e) = out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        shared.stop.store(true, Ordering::Relaxed);
        tracing::debug!(error = %e, "upgrade progress output failed; hiding it");
    }
}

/// One frame: `⠹ Downloading v0.6.11  4.2 / 9.8 MB  43%`.
pub(crate) fn progress_line(frame: usize, stage: &str, done: u64, total: Option<u64>) -> String {
    let spinner = FRAMES[frame % FRAMES.len()];
    let bytes = match (done, total) {
        (0, None) => String::new(),
        (_, Some(total)) if total > 0 => format!(
            "  {} / {}  {}%",
            mb(done),
            mb(total),
            (done.min(total) * 100) / total
        ),
        _ => format!("  {}", mb(done)),
    };
    format!("{spinner} {stage}{bytes}")
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[cfg(test)]
#[path = "upgrade_progress_tests.rs"]
mod tests;
