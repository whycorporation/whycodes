use std::time::{Duration, Instant};

use super::*;

const ROWS: u16 = 40;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Tick every 16 ms from `from` until the gesture ends; sum the rows.
fn drain(w: &mut WheelScroll, from: Instant) -> (isize, Instant) {
    let mut now = from;
    let mut total = 0;
    for _ in 0..200 {
        if !w.is_active() {
            return (total, now);
        }
        now += FLUSH_CADENCE;
        total += w.tick(now, ROWS);
    }
    panic!("wheel stream never ended");
}

#[test]
fn events_per_notch_reads_the_terminal() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).to_string())
        }
    };
    assert_eq!(events_per_notch(env(&[("WT_SESSION", "x")])), 1);
    assert_eq!(events_per_notch(env(&[("TERM_PROGRAM", "vscode")])), 1);
    assert_eq!(events_per_notch(env(&[("TERM_PROGRAM", "Cursor")])), 1);
    assert_eq!(events_per_notch(env(&[("TERM_PROGRAM", "ghostty")])), 3);
    assert_eq!(events_per_notch(env(&[])), 3);
}

#[test]
fn notch_on_a_three_event_terminal_is_three_lines_at_once() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(3);
    let mut rows = 0;
    for _ in 0..3 {
        rows += w.on_event(t0, true, ROWS);
    }
    assert_eq!(rows, 3, "promotion to wheel flushes the whole notch");
    let (rest, _) = drain(&mut w, t0);
    assert_eq!(rest, 0);
}

#[test]
fn notch_on_a_one_event_terminal_is_three_lines() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(1);
    assert_eq!(w.on_event(t0, false, ROWS), -3);
    let (rest, _) = drain(&mut w, t0);
    assert_eq!(rest, 0);
}

#[test]
fn slow_notches_each_scroll_and_the_stream_ends_after_the_gap() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(1);
    let mut rows = w.on_event(t0, true, ROWS);
    rows += w.on_event(t0 + ms(50), true, ROWS);
    rows += w.on_event(t0 + ms(100), true, ROWS);
    let (rest, _) = drain(&mut w, t0 + ms(100));
    assert_eq!(rows + rest, 9);
    assert!(!w.is_active());
}

#[test]
fn a_flood_is_paced_across_frames_not_dumped_at_once() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(3);
    // Sixty events drained in one batch (a fast flick).
    let first: isize = (0..60).map(|_| w.on_event(t0, true, ROWS)).sum();
    let cap = flush_cap(ROWS) as isize;
    assert!(first <= cap, "one frame moved {first} > cap {cap}");
    // Further frames each stay under the cap too.
    let mut now = t0;
    let mut total = first;
    while w.is_active() {
        now += FLUSH_CADENCE;
        let step = w.tick(now, ROWS);
        assert!(step.abs() <= cap);
        total += step;
    }
    assert!(total > cap, "the backlog kept moving after the first frame");
    assert!(total <= 60, "never more than the flood earned");
}

#[test]
fn coast_tapers_and_is_budgeted_at_one_cap() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(3);
    for _ in 0..30 {
        w.on_event(t0, true, ROWS);
    }
    let cap = flush_cap(ROWS) as isize;
    // First tick still delivers live input (capped); coasting starts after.
    let mut now = t0 + FLUSH_CADENCE;
    assert_eq!(w.tick(now, ROWS), cap);
    let mut coast = Vec::new();
    while w.is_active() {
        now += FLUSH_CADENCE;
        let step = w.tick(now, ROWS);
        if step != 0 {
            coast.push(step);
        }
    }
    assert!(coast.iter().sum::<isize>() <= cap, "coast {coast:?}");
    assert!(
        coast.windows(2).all(|p| p[1] <= p[0]),
        "coast must decelerate: {coast:?}"
    );
}

#[test]
fn direction_flip_drops_the_old_backlog() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(3);
    for _ in 0..60 {
        w.on_event(t0, true, ROWS);
    }
    let down = w.on_event(t0 + ms(20), false, ROWS);
    assert_eq!(down, -1, "the reversal answers at once, the new way");
    let (rest, _) = drain(&mut w, t0 + ms(20));
    assert!(rest <= 0, "no stale upward rows after the flip: {rest}");
}

#[test]
fn trackpad_is_detected_on_one_event_terminals_and_accelerates() {
    let t0 = Instant::now();
    // Slow trackpad-ish stream: 25 ms apart → detected, base rate.
    let mut slow = WheelScroll::new(1);
    let mut slow_rows = 0;
    for i in 0..20 {
        slow_rows += slow.on_event(t0 + ms(25 * i), true, ROWS);
    }
    let (rest, _) = drain(&mut slow, t0 + ms(25 * 19));
    slow_rows += rest;

    // Same count 7 ms apart → fast band.
    let mut fast = WheelScroll::new(1);
    let mut fast_rows = 0;
    for i in 0..20 {
        fast_rows += fast.on_event(t0 + ms(7 * i), true, ROWS);
    }
    let (rest, _) = drain(&mut fast, t0 + ms(7 * 19));
    fast_rows += rest;

    assert!(
        slow_rows < 60,
        "trackpad must not price each event as a notch"
    );
    assert!(
        fast_rows > slow_rows,
        "fast {fast_rows} vs slow {slow_rows}"
    );
}

#[test]
fn trackpad_remainder_carries_only_in_the_same_direction() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(1);
    for i in 0..5 {
        w.on_event(t0 + ms(10 * i), true, ROWS);
    }
    let (_, end) = drain(&mut w, t0 + ms(40));
    let carry = w.carry;
    assert!((0.0..1.0).contains(&carry));
    // A new downward gesture starts clean.
    w.on_event(end + ms(200), false, ROWS);
    assert_eq!(w.carry, 0.0);
}

#[test]
fn a_new_gesture_after_the_gap_flushes_the_old_backlog_first() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(3);
    let mut rows = 0;
    for _ in 0..30 {
        rows += w.on_event(t0, true, ROWS);
    }
    // Next event lands after the gap without any tick in between.
    rows += w.on_event(t0 + ms(100), true, ROWS);
    let (rest, _) = drain(&mut w, t0 + ms(100));
    assert!(rows + rest > flush_cap(ROWS) as isize);
}

#[test]
fn cancel_ends_the_gesture() {
    let t0 = Instant::now();
    let mut w = WheelScroll::new(3);
    for _ in 0..30 {
        w.on_event(t0, true, ROWS);
    }
    w.cancel();
    assert!(!w.is_active());
    assert_eq!(w.tick(t0 + ms(16), ROWS), 0);
}

#[test]
fn tick_without_a_stream_or_before_cadence_is_a_no_op() {
    let t0 = Instant::now();
    let mut w = WheelScroll::default();
    assert_eq!(w.tick(t0, ROWS), 0);
    for _ in 0..30 {
        w.on_event(t0, true, ROWS);
    }
    assert_eq!(w.tick(t0 + ms(1), ROWS), 0, "inside the 16 ms cadence");
    assert!(w.is_active());
}

#[test]
fn tiny_viewport_still_uses_the_floor_cap() {
    assert_eq!(flush_cap(4), MIN_FLUSH_CAP);
    assert_eq!(flush_cap(40), 20);
}

#[test]
fn medium_interval_accelerates_between_bands() {
    let t0 = Instant::now();
    let mut s = Stream::new(t0, true);
    s.push(t0, 3);
    s.push(t0 + ms(14), 3);
    let a = s.accel();
    assert!(a > ACCEL_MEDIUM && a < ACCEL_FAST, "{a}");
    s.push(t0 + ms(64), 3);
    s.push(t0 + ms(114), 3);
    s.push(t0 + ms(164), 3);
    assert!(s.accel() < ACCEL_FAST);
}
