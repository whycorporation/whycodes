//! Wheel / trackpad scroll normalization — a trimmed port of Grok Build's
//! `xai-grok-pager-render/src/input/mouse.rs` (Apache-2.0).
//!
//! Terminal wheel events carry a direction, not a magnitude, and terminals
//! disagree on how many events one notch sends. Summing a drained batch and
//! applying it in one frame made a trackpad flick teleport hundreds of rows.
//!
//! Instead events form a *stream* (one gesture, ended by an 80 ms gap or a
//! direction flip). The stream is priced in lines and delivered at most one
//! capped chunk per 16 ms flush, so a fling animates instead of jumping:
//!
//! - Wheel: one notch is [`LINES_PER_NOTCH`] lines, whatever the terminal's
//!   events-per-notch.
//! - Trackpad: one line per standard event, accelerated by event rate. The
//!   sub-line remainder carries into the next same-direction gesture.
//! - After input stops, the backlog tapers (halves per flush) and is
//!   budgeted at one cap, so a gesture never keeps sliding.
//!
//! Callers must [`WheelScroll::tick`] at [`FLUSH_CADENCE`] while
//! [`WheelScroll::is_active`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Flush cadence while a stream is active (~60 fps).
pub const FLUSH_CADENCE: Duration = Duration::from_millis(16);
/// Quiet time that ends a gesture.
const STREAM_GAP: Duration = Duration::from_millis(80);
const LINES_PER_NOTCH: f32 = 3.0;
/// Events per notch on most terminals. Trackpad pricing always uses it so
/// every terminal gets the same base trackpad rate.
const STANDARD_EVENTS_PER_NOTCH: u8 = 3;
/// A whole notch arriving this fast is a wheel (multi-event terminals).
const WHEEL_DETECT: Duration = Duration::from_millis(12);
/// Single-event terminals: faster than this on average is a trackpad.
const TRACKPAD_DETECT_MAX_INTERVAL_MS: f32 = 30.0;
/// Floor for the per-flush cap (half the viewport).
const MIN_FLUSH_CAP: i32 = 6;
const ACCEL_FAST_MS: f32 = 8.0;
const ACCEL_MEDIUM_MS: f32 = 20.0;
/// Sub-6 ms spacing is terminal batching, not gesture speed.
const ACCEL_MIN_INTERVAL_MS: f32 = 6.0;
const ACCEL_FAST: f32 = 2.5;
const ACCEL_MEDIUM: f32 = 1.6;
const ACCEL_HISTORY: usize = 6;

/// Events one wheel notch sends on this terminal. Windows Terminal and the
/// VS Code family send one; most others send three.
pub fn events_per_notch(var: impl Fn(&str) -> Option<String>) -> u8 {
    if var("WT_SESSION").is_some() {
        return 1;
    }
    let program = var("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    match program.as_str() {
        "vscode" | "cursor" | "windsurf" | "zed" => 1,
        _ => STANDARD_EVENTS_PER_NOTCH,
    }
}

fn flush_cap(viewport_rows: u16) -> i32 {
    (i32::from(viewport_rows) / 2).max(MIN_FLUSH_CAP)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Unknown,
    Wheel,
    Trackpad,
}

/// One gesture. Magnitudes are unsigned; `up` signs the output.
#[derive(Debug)]
struct Stream {
    start: Instant,
    last: Instant,
    up: bool,
    events: u32,
    /// Sum of each event's acceleration multiplier at arrival, so a
    /// slowing gesture never shrinks what earlier events earned.
    weighted: f32,
    applied: i32,
    kind: Kind,
    just_promoted: bool,
    intervals: VecDeque<f32>,
    events_at_flush: u32,
    coast_spent: i32,
}

impl Stream {
    fn new(now: Instant, up: bool) -> Self {
        Self {
            start: now,
            last: now,
            up,
            events: 0,
            weighted: 0.0,
            applied: 0,
            kind: Kind::Unknown,
            just_promoted: false,
            intervals: VecDeque::with_capacity(ACCEL_HISTORY + 1),
            events_at_flush: 0,
            coast_spent: 0,
        }
    }

    fn push(&mut self, now: Instant, ept: u8) {
        let ms = now.duration_since(self.last).as_secs_f32() * 1000.0;
        if self.events > 0 && ms >= ACCEL_MIN_INTERVAL_MS {
            self.intervals.push_back(ms);
            if self.intervals.len() > ACCEL_HISTORY {
                self.intervals.pop_front();
            }
        }
        self.last = now;
        self.events += 1;
        self.weighted += self.accel();
        if self.kind != Kind::Unknown {
            return;
        }
        if ept <= 1 {
            if self.events > 2
                && self
                    .avg_interval()
                    .is_some_and(|avg| avg < TRACKPAD_DETECT_MAX_INTERVAL_MS)
            {
                self.kind = Kind::Trackpad;
            }
        } else if self.events >= u32::from(ept) && now.duration_since(self.start) <= WHEEL_DETECT {
            self.kind = Kind::Wheel;
            self.just_promoted = true;
        }
    }

    fn avg_interval(&self) -> Option<f32> {
        if self.intervals.is_empty() {
            return None;
        }
        Some(self.intervals.iter().sum::<f32>() / self.intervals.len() as f32)
    }

    fn accel(&self) -> f32 {
        match self.avg_interval() {
            Some(avg) if avg <= ACCEL_FAST_MS => ACCEL_FAST,
            Some(avg) if avg <= ACCEL_MEDIUM_MS => {
                let t = (avg - ACCEL_FAST_MS) / (ACCEL_MEDIUM_MS - ACCEL_FAST_MS);
                ACCEL_FAST + t * (ACCEL_MEDIUM - ACCEL_FAST)
            }
            _ => 1.0,
        }
    }

    /// Lines this gesture has earned so far, fractional.
    fn desired(&self, ept: u8, carry: f32) -> f32 {
        if self.kind == Kind::Trackpad {
            self.weighted * (LINES_PER_NOTCH / f32::from(STANDARD_EVENTS_PER_NOTCH)) + carry
        } else {
            self.events as f32 * (LINES_PER_NOTCH / f32::from(ept.max(1)))
        }
    }

    fn wheel_like(&self, ept: u8) -> bool {
        self.kind == Kind::Wheel || (self.kind == Kind::Unknown && ept <= 1)
    }

    /// Whole lines owed. Never negative: a re-price below what was already
    /// applied is a pause, not a bounce.
    fn pending(&self, ept: u8, carry: f32) -> i32 {
        let mut whole = self.desired(ept, carry).trunc() as i32;
        if whole == 0 && self.wheel_like(ept) {
            whole = 1;
        }
        (whole - self.applied).max(0)
    }

    /// No input since the last delivering flush: what remains is catch-up.
    fn coasting(&self) -> bool {
        self.events == self.events_at_flush
    }

    /// Lines a flush right now may deliver.
    fn flushable(&self, ept: u8, carry: f32, cap: i32) -> i32 {
        let pending = self.pending(ept, carry);
        if self.coasting() {
            let taper = (pending / 2).max(LINES_PER_NOTCH as i32);
            pending.min(taper).min((cap - self.coast_spent).max(0))
        } else {
            pending.min(cap)
        }
    }
}

/// Per-app wheel state. Output rows are signed like
/// [`crate::app::TuiApp::scroll_rows`]: positive scrolls up (older).
#[derive(Debug)]
pub struct WheelScroll {
    events_per_notch: u8,
    stream: Option<Stream>,
    last_flush: Option<Instant>,
    /// Sub-line trackpad remainder and the direction it belongs to.
    carry: f32,
    carry_up: Option<bool>,
}

impl Default for WheelScroll {
    fn default() -> Self {
        Self::new(STANDARD_EVENTS_PER_NOTCH)
    }
}

impl WheelScroll {
    pub fn new(events_per_notch: u8) -> Self {
        Self {
            events_per_notch: events_per_notch.max(1),
            stream: None,
            last_flush: None,
            carry: 0.0,
            carry_up: None,
        }
    }

    /// Detect events-per-notch from the process environment.
    pub fn from_env() -> Self {
        Self::new(events_per_notch(|k| std::env::var(k).ok()))
    }

    /// True while a gesture is live or still draining.
    pub fn is_active(&self) -> bool {
        self.stream.is_some()
    }

    /// Drop the gesture (a modal took the wheel).
    pub fn cancel(&mut self) {
        self.stream = None;
        self.carry = 0.0;
        self.carry_up = None;
    }

    /// Feed one wheel event. Returns rows to scroll now.
    pub fn on_event(&mut self, now: Instant, up: bool, viewport_rows: u16) -> isize {
        let ept = self.events_per_notch;
        let cap = flush_cap(viewport_rows);
        let mut lines = 0;
        if let Some(mut s) = self.stream.take() {
            if s.up != up {
                // Reversal is instant: the old backlog is dropped, not played.
                self.finish(&s);
            } else if now.duration_since(s.last) > STREAM_GAP {
                lines += self.flush(&mut s, now, cap);
                self.finish(&s);
            } else {
                self.stream = Some(s);
            }
        }
        let mut s = match self.stream.take() {
            Some(s) => s,
            None => {
                if self.carry_up != Some(up) {
                    self.carry = 0.0;
                    self.carry_up = Some(up);
                }
                Stream::new(now, up)
            }
        };
        s.push(now, ept);
        // A gesture's first event answers at once (a reversal included);
        // the rest of a flood waits for the cadence.
        if s.events == 1 || self.cadence_due(now) || s.just_promoted {
            s.just_promoted = false;
            lines += self.flush(&mut s, now, cap);
        }
        self.stream = Some(s);
        signed(lines, up)
    }

    /// Flush on cadence; end the gesture once it is quiet and drained.
    pub fn tick(&mut self, now: Instant, viewport_rows: u16) -> isize {
        let Some(mut s) = self.stream.take() else {
            return 0;
        };
        let cap = flush_cap(viewport_rows);
        let quiet = now.duration_since(s.last) > STREAM_GAP;
        if quiet && s.flushable(self.events_per_notch, self.carry, cap) == 0 {
            self.finish(&s);
            return 0;
        }
        let lines = if self.cadence_due(now) {
            self.flush(&mut s, now, cap)
        } else {
            0
        };
        let up = s.up;
        self.stream = Some(s);
        signed(lines, up)
    }

    fn cadence_due(&self, now: Instant) -> bool {
        self.last_flush
            .is_none_or(|at| now.duration_since(at) >= FLUSH_CADENCE)
    }

    fn flush(&mut self, s: &mut Stream, now: Instant, cap: i32) -> i32 {
        let lines = s.flushable(self.events_per_notch, self.carry, cap);
        // A zero flush must not move the clock, or the cadence never fires.
        if lines == 0 {
            return 0;
        }
        if s.coasting() {
            s.coast_spent += lines;
        }
        s.applied += lines;
        s.events_at_flush = s.events;
        self.last_flush = Some(now);
        lines
    }

    /// Keep only a trackpad's fractional remainder; whole-line backlog
    /// would hand the next gesture a burst it did not earn.
    fn finish(&mut self, s: &Stream) {
        self.carry = if s.kind == Kind::Trackpad {
            (s.desired(self.events_per_notch, self.carry) - s.applied as f32)
                .fract()
                .max(0.0)
        } else {
            0.0
        };
    }
}

fn signed(lines: i32, up: bool) -> isize {
    let lines = lines as isize;
    if up { lines } else { -lines }
}

#[cfg(test)]
#[path = "wheel_tests.rs"]
mod tests;
