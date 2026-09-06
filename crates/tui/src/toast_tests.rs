use super::*;

#[test]
fn a_toast_expires_after_its_ttl() {
    let now = Instant::now();
    let toast = Toast::at(now, ToastKind::Info, "hello");
    assert!(!toast.is_expired(now));
    assert!(!toast.is_expired(now + DEFAULT_TTL - Duration::from_millis(1)));
    assert!(toast.is_expired(now + DEFAULT_TTL));
}

#[test]
fn errors_stay_up_longer_than_everything_else() {
    let now = Instant::now();
    let info = Toast::at(now, ToastKind::Info, "x");
    let error = Toast::at(now, ToastKind::Error, "x");
    assert!(info.is_expired(now + DEFAULT_TTL));
    assert!(!error.is_expired(now + DEFAULT_TTL));
    assert!(error.is_expired(now + ERROR_TTL));
}

#[test]
fn warnings_outlast_info() {
    let now = Instant::now();
    let info = Toast::at(now, ToastKind::Info, "x");
    let warn = Toast::at(now, ToastKind::Warning, "x");
    assert!(info.is_expired(now + DEFAULT_TTL));
    assert!(!warn.is_expired(now + DEFAULT_TTL));
    assert!(warn.is_expired(now + WARNING_TTL));
}

#[test]
fn pruning_removes_only_the_expired_ones() {
    let now = Instant::now();
    let mut toasts = Toasts::default();
    toasts.push_toast(Toast::at(now, ToastKind::Info, "old"));
    toasts.push_toast(Toast::at(now + DEFAULT_TTL, ToastKind::Info, "new"));

    toasts.prune(now + DEFAULT_TTL);
    assert_eq!(toasts.visible().len(), 1);
    assert_eq!(toasts.visible()[0].message, "new");
}

#[test]
fn a_burst_drops_the_oldest_not_the_newest() {
    let mut toasts = Toasts::default();
    for i in 0..MAX_VISIBLE + 2 {
        toasts.push(ToastKind::Info, format!("m{i}"));
    }
    assert_eq!(toasts.visible().len(), MAX_VISIBLE);
    // The two oldest are gone; the most recent is kept, because it is the
    // one the user just caused.
    assert_eq!(toasts.visible()[0].message, "m2");
    assert_eq!(
        toasts.visible().last().unwrap().message,
        format!("m{}", MAX_VISIBLE + 1)
    );
}

#[test]
fn every_kind_has_a_distinct_glyph() {
    let glyphs: Vec<&str> = [
        ToastKind::Info,
        ToastKind::Success,
        ToastKind::Warning,
        ToastKind::Error,
    ]
    .iter()
    .map(|k| k.glyph())
    .collect();
    let mut unique = glyphs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        glyphs.len(),
        "kinds must be distinguishable without colour"
    );
}

#[test]
fn an_empty_stack_reports_itself_empty() {
    let mut toasts = Toasts::default();
    assert!(toasts.is_empty());
    toasts.push(ToastKind::Info, "x");
    assert!(!toasts.is_empty());
}
