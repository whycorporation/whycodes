use super::*;

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&lock(&self.0)).into_owned()
    }
}

#[test]
fn progress_line_shows_bytes_and_percent() {
    assert_eq!(progress_line(0, "Checking", 0, None), "⠋ Checking");
    assert_eq!(
        progress_line(2, "Downloading v0.6.11", 4_200_000, Some(9_800_000)),
        "⠹ Downloading v0.6.11  4.2 MB / 9.8 MB  42%"
    );
    assert_eq!(
        progress_line(10, "Downloading", 1_500_000, None),
        "⠋ Downloading  1.5 MB"
    );
    // More bytes than announced never shows over 100%.
    assert!(progress_line(0, "x", 20, Some(10)).ends_with("100%"));
    // A zero length is treated as unknown rather than dividing by it.
    assert_eq!(progress_line(0, "x", 0, Some(0)), "⠋ x  0.0 MB");
}

#[test]
fn animated_progress_redraws_one_line_and_clears_it() {
    let buf = Buf::default();
    let progress = Progress::to(Box::new(buf.clone()), true);
    std::thread::sleep(TICK * 2);
    assert!(
        buf.text().is_empty(),
        "nothing drawn before the first stage"
    );
    progress.stage("Downloading v9.9.9");
    progress.set_total(Some(2_000_000));
    progress.advance(1_000_000);
    std::thread::sleep(TICK * 3);
    progress.finish();
    let text = buf.text();
    assert!(
        text.contains("Downloading v9.9.9  1.0 MB / 2.0 MB  50%"),
        "{text:?}"
    );
    assert!(text.contains('\r'), "redraws in place: {text:?}");
    assert!(
        text.ends_with("\r\x1b[2K"),
        "line cleared at the end: {text:?}"
    );
    assert!(!text.contains('\n'), "never scrolls: {text:?}");
}

#[test]
fn plain_progress_prints_each_stage_once() {
    let buf = Buf::default();
    let progress = Progress::to(Box::new(buf.clone()), false);
    progress.stage("Downloading v9.9.9");
    progress.set_total(None);
    progress.advance(10);
    progress.stage("Verifying checksum");
    drop(progress);
    assert_eq!(buf.text(), "  Downloading v9.9.9…\n  Verifying checksum…\n");
}

#[test]
fn silent_and_stderr_progress_do_not_panic() {
    let silent = Progress::silent();
    silent.stage("x");
    silent.set_total(Some(1));
    silent.advance(1);
    silent.finish();
    // Under `cargo test` stderr is not a terminal: plain mode, no ticker.
    let progress = Progress::stderr();
    assert!(progress.ticker.is_none() || progress.animate);
    progress.finish();
}
