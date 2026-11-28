//! The terminal the loop draws on: opening the console, raw mode and the
//! alternate screen, reading events, drawing, and putting it all back.
use super::*;

/// Concrete writer for ratatui/crossterm (`execute!` needs `Sized`).
pub(super) enum TuiWriter {
    /// Buffered: crossterm queues a write per changed cell (~11k for a full
    /// 120×40 frame). Unbuffered, each was a console `WriteFile`; now a frame
    /// leaves in one write when ratatui / `execute!` flush.
    Console(io::BufWriter<std::fs::File>),
    Stdout(io::Stdout),
    /// In-memory sink so `LoopTerm::live` can be unit-tested without a TTY.
    Buf(Vec<u8>),
    /// Always-failing sink so `Terminal::new` / alt-screen `execute!` error
    /// arms run without a real TTY.
    #[allow(dead_code)]
    Fail,
}

impl Write for TuiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Console(f) => f.write(buf),
            Self::Stdout(s) => s.write(buf),
            Self::Buf(b) => b.write(buf),
            Self::Fail => Err(io::Error::other("tui writer fail")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Console(f) => f.flush(),
            Self::Stdout(s) => s.flush(),
            Self::Buf(b) => b.flush(),
            Self::Fail => Err(io::Error::other("tui writer fail")),
        }
    }
}

pub(super) struct LoopIo {
    scripted: Option<VecDeque<Event>>,
    crossterm: VecDeque<Event>,
    poll_err: bool,
    read_err: bool,
    force_zero_poll: bool,
}

impl LoopIo {
    pub(super) fn from_inject(inject: &mut LoopInject) -> Self {
        let scripted = inject.scripted_events.take();
        let crossterm = std::mem::take(&mut inject.crossterm_events);
        Self {
            // Stub / live-buf / scripted runs never have a TTY. After the
            // injected queue drains, skip OS poll (ENOENT/EAGAIN on CI).
            force_zero_poll: inject.live_buf || scripted.is_some() || !crossterm.is_empty(),
            scripted,
            crossterm,
            poll_err: inject.poll_err,
            read_err: inject.read_err,
        }
    }

    pub(super) fn is_headless(&self) -> bool {
        self.scripted.is_some()
    }

    pub(super) fn crossterm_empty(&self) -> bool {
        self.crossterm.is_empty()
    }

    pub(super) fn peek(&self) -> Option<&Event> {
        self.scripted.as_ref().and_then(|q| q.front())
    }

    pub(super) fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if let Some(q) = &self.scripted {
            return Ok(!q.is_empty());
        }
        self.poll_crossterm(timeout)
    }

    pub(super) fn read_batch(&mut self) -> io::Result<Vec<Event>> {
        if let Some(q) = &mut self.scripted {
            // One event per poll so a scripted Enter can start a turn before
            // later keys (Esc, :q) are applied.
            match q.pop_front() {
                Some(ev) => Ok(vec![ev]),
                None => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "scripted TUI events exhausted",
                )),
            }
        } else {
            self.read_event_batch()
        }
    }

    pub(super) fn poll_crossterm(&mut self, timeout: Duration) -> io::Result<bool> {
        if self.poll_err {
            self.poll_err = false;
            return Err(io::Error::other("crossterm stub poll failed"));
        }
        if !self.crossterm.is_empty() {
            return Ok(true);
        }
        if self.force_zero_poll {
            return Ok(false);
        }
        live_poll_crossterm(timeout)
    }

    pub(super) fn read_crossterm(&mut self) -> io::Result<Event> {
        if self.read_err {
            self.read_err = false;
            return Err(io::Error::other("crossterm stub read failed"));
        }
        if let Some(ev) = self.crossterm.pop_front() {
            return Ok(ev);
        }
        if self.force_zero_poll {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "crossterm stub empty",
            ));
        }
        live_read_crossterm()
    }

    pub(super) fn read_event_batch(&mut self) -> io::Result<Vec<Event>> {
        const MAX_BATCH: usize = 256;
        // PowerShell / ConPTY often delivers an unbracketed paste one key
        // per poll. After draining the ready queue, keep waiting 12 ms for
        // the next key **while the flood continues**, so the whole paste
        // lands in one batch → one insert, one paint (no typewriter echo)
        // and `coalesce_unbracketed_paste` can fold it. Bounds: humans type
        // with >12 ms gaps (one linger miss ends the loop), a mouse/resize
        // event ends it (drag-select must not lag), LINGER_MAX_BATCH caps
        // memory, and the 400 ms deadline caps paint starvation. Skip on
        // injected/scripted queues so tests do not sleep.
        let mut batch = Vec::with_capacity(8);
        batch.push(self.read_crossterm()?);
        while batch.len() < MAX_BATCH {
            match self.poll_crossterm(Duration::ZERO) {
                Ok(true) => batch.push(self.read_crossterm()?),
                Ok(false) => break,
                Err(e) => return Err(e),
            }
        }
        maybe_linger_windows_paste(cfg!(windows), self.force_zero_poll, self, batch)
    }
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) trait CrosstermIo {
    fn poll_crossterm(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read_crossterm(&mut self) -> io::Result<Event>;
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
impl CrosstermIo for LoopIo {
    fn poll_crossterm(&mut self, timeout: Duration) -> io::Result<bool> {
        LoopIo::poll_crossterm(self, timeout)
    }

    fn read_crossterm(&mut self) -> io::Result<Event> {
        LoopIo::read_crossterm(self)
    }
}

/// Windows ConPTY paste linger: keep polling while keys arrive with ≤12 ms
/// gaps. Extracted so Linux CI can drive the loop without a live TTY.
pub(super) fn should_linger_windows_paste(
    is_windows: bool,
    force_zero_poll: bool,
    batch: &[Event],
) -> bool {
    is_windows && !force_zero_poll && batch.iter().all(|e| matches!(e, Event::Key(_)))
}

pub(super) fn maybe_linger_windows_paste(
    is_windows: bool,
    force_zero_poll: bool,
    io: &mut impl CrosstermIo,
    batch: Vec<Event>,
) -> io::Result<Vec<Event>> {
    if !should_linger_windows_paste(is_windows, force_zero_poll, &batch) {
        return Ok(batch);
    }
    linger_windows_paste_batch(io, batch)
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) fn linger_windows_paste_batch(
    io: &mut impl CrosstermIo,
    mut batch: Vec<Event>,
) -> io::Result<Vec<Event>> {
    const PASTE_LINGER: Duration = Duration::from_millis(12);
    const PASTE_LINGER_TOTAL: Duration = Duration::from_millis(400);
    const LINGER_MAX_BATCH: usize = 4096;
    let deadline = std::time::Instant::now() + PASTE_LINGER_TOTAL;
    'linger: while batch.len() < LINGER_MAX_BATCH && std::time::Instant::now() < deadline {
        if !io.poll_crossterm(PASTE_LINGER)? {
            break;
        }
        loop {
            let ev = io.read_crossterm()?;
            let is_key = matches!(ev, Event::Key(_));
            batch.push(ev);
            if !is_key || batch.len() >= LINGER_MAX_BATCH {
                break 'linger;
            }
            match io.poll_crossterm(Duration::ZERO) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(batch)
}

pub(super) fn paste_enter_guard_for(is_windows: bool, headless: bool) -> bool {
    is_windows && !headless
}

pub(super) enum LoopTerm {
    Live(Terminal<QuantizingBackend<CrosstermBackend<TuiWriter>>>),
    Headless(Terminal<QuantizingBackend<TestBackend>>),
}

impl LoopTerm {
    pub(super) fn live(out: TuiWriter, color_mode: ColorMode) -> anyhow::Result<Self> {
        let backend = QuantizingBackend::with_size_fallback(
            CrosstermBackend::new(out),
            color_mode,
            ratatui::layout::Size {
                width: 80,
                height: 24,
            },
        );
        Ok(Self::Live(
            Terminal::new(backend).inspect_err(on_terminal_new_failed)?,
        ))
    }

    pub(super) fn headless(color_mode: ColorMode) -> anyhow::Result<Self> {
        let backend = QuantizingBackend::new(TestBackend::new(80, 24), color_mode);
        Ok(Self::Headless(Terminal::new(backend)?))
    }

    pub(super) fn resize(&mut self, area: Rect) {
        match self {
            Self::Live(t) => {
                if let Err(e) = t.resize(area) {
                    log_resize_failed("live", e);
                }
            }
            // The test backend's error type is uninhabited.
            Self::Headless(t) => {
                let Ok(()) = t.resize(area);
            }
        }
    }

    /// Drop the last painted buffer so the next `draw` diffs against empty
    /// (every cell is sent) without CSI erase. Windows PowerShell paints
    /// `ClearType::All` with the profile default background — a white flash.
    ///
    /// Hide the caret first: a full cell dump with a visible cursor looks
    /// like the bar sweeping the screen top-to-bottom (conhost / WT).
    pub(super) fn reset_prev_for_full_redraw(&mut self) {
        match self {
            Self::Live(t) => {
                if let Err(e) = t.hide_cursor() {
                    tracing::debug!(error = %e, "hide cursor before full redraw failed");
                }
                t.current_buffer_mut().reset();
                t.swap_buffers();
            }
            Self::Headless(t) => {
                if let Err(e) = t.hide_cursor() {
                    tracing::debug!(error = %e, "hide cursor before full redraw failed");
                }
                t.current_buffer_mut().reset();
                t.swap_buffers();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn current_buffer_mut_for_test(&mut self) -> &mut ratatui::buffer::Buffer {
        match self {
            Self::Live(t) => t.current_buffer_mut(),
            Self::Headless(t) => t.current_buffer_mut(),
        }
    }

    #[cfg(test)]
    pub(super) fn swap_buffers_for_test(&mut self) {
        match self {
            Self::Live(t) => t.swap_buffers(),
            Self::Headless(t) => t.swap_buffers(),
        }
    }

    pub(super) fn draw_app(
        &mut self,
        app: &mut TuiApp,
        fail: &mut bool,
    ) -> anyhow::Result<(ratatui::layout::Rect, Option<crate::cell_grid::CellGrid>)> {
        if *fail {
            *fail = false;
            return Err(anyhow::anyhow!("tui draw failed"));
        }
        match self {
            Self::Live(t) => {
                // Hide + CSI ?2026 before the cell dump. ratatui only hides
                // the cursor *after* flush, so a FocusGained full redraw
                // otherwise walks the hardware caret down every row.
                sync_window_title(t.backend_mut(), app);
                begin_cell_dump(t.backend_mut());
                let result = draw_into(t, app);
                end_cell_dump(t.backend_mut());
                result
            }
            Self::Headless(t) => {
                if let Err(e) = t.hide_cursor() {
                    tracing::debug!(error = %e, "hide cursor before headless draw failed");
                }
                draw_into(t, app)
            }
        }
    }

    pub(super) fn draw_splash(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Live(t) => {
                // Same guard as `draw_app`: the splash fills every cell and
                // is the first paint after alt-screen. A visible caret walks
                // the dump on Windows PowerShell (home screen, not chat).
                begin_cell_dump(t.backend_mut());
                let result = t.draw(render_splash).map(|_| ()).map_err(Into::into);
                end_cell_dump(t.backend_mut());
                result
            }
            Self::Headless(t) => {
                if let Err(e) = t.hide_cursor() {
                    tracing::debug!(error = %e, "hide cursor before splash failed");
                }
                t.draw(render_splash).map(|_| ()).map_err(Into::into)
            }
        }
    }

    pub(super) fn restore(self, keyboard_enhanced: bool) {
        match self {
            Self::Live(mut terminal) => {
                restore_live_backend(terminal.backend_mut(), keyboard_enhanced);
                let _ = terminal.show_cursor();
            }
            Self::Headless(mut terminal) => {
                let _ = terminal.show_cursor();
            }
        }
    }
}

pub(super) fn on_terminal_new_failed(e: &impl std::fmt::Display) {
    let _ = disable_raw_mode();
    windows_console::restore_windows_console();
    whycodes_core::logging::emit(
        "whycodes_tui",
        "error",
        "tui.terminal_new_failed",
        Some(serde_json::json!({ "error": e.to_string() })),
    );
}

pub(super) fn log_resize_failed(kind: &str, e: impl std::fmt::Display) {
    tracing::debug!(error = %e, "{kind} terminal resize failed");
}

/// Start a synchronized frame, then hide the caret, before a cell dump.
///
/// Windows PowerShell / conhost keep the hardware cursor visible while
/// ratatui `MoveTo`+`Print`s every cell. That reads as the blinking bar
/// sweeping top-to-bottom on first paint (splash / empty home) and on
/// tab-switch (FocusGained full redraw). `CSI ?2026` is ignored on hosts
/// that do not implement it.
///
/// Order matters: `BeginSynchronizedUpdate` must come **before** `Hide` so
/// the hide → cells → show cycle sits inside one atomic frame. With `Hide`
/// first, hosts that honour `?2026` present the caret-off state at the top
/// of every draw — during streaming (~25 fps) the prompt caret visibly
/// strobes. Hosts that ignore `?2026` still get `Hide` ahead of the cell
/// writes, so the anti-sweep behaviour is unchanged.
/// XTWINOPS title stack: save the shell's title on entry, restore it on
/// exit. Hosts without a title stack ignore both; Windows also restores the
/// console title directly (`windows_console`).
pub(super) const TITLE_PUSH: &str = "[22;0t";

pub(super) const TITLE_POP: &str = "[23;0t";

/// Terminal window / tab title: `whycodes · <session title>`, or `whycodes`
/// while the session still has its placeholder name.
pub(super) fn window_title(session_title: &str, project_dir: &std::path::Path) -> String {
    let title = whycodes_session::title::sanitize_title(session_title);
    if title.is_empty() || whycodes_session::title::looks_like_default_title(&title, project_dir) {
        return "whycodes".into();
    }
    format!("whycodes · {title}")
}

/// Own write so a failed mouse/paste teardown cannot skip the title restore.
pub(super) fn pop_title(out: &mut impl Write) {
    if let Err(e) = execute!(out, Print(TITLE_POP)) {
        tracing::debug!(error = %e, "restore terminal title failed");
    }
}

/// Set the terminal title when the session title changed since the last frame.
pub(super) fn sync_window_title(out: &mut impl Write, app: &mut TuiApp) {
    if app.window_title_for.as_deref() == Some(app.session_title.as_str()) {
        return;
    }
    let title = window_title(&app.session_title, &app.project_dir);
    if let Err(e) = execute!(out, SetTitle(&title)) {
        tracing::debug!(error = %e, "set terminal title failed");
    }
    app.window_title_for = Some(app.session_title.clone());
}

pub(super) fn begin_cell_dump(out: &mut impl Write) {
    if let Err(e) = execute!(out, BeginSynchronizedUpdate, Hide) {
        tracing::debug!(error = %e, "begin cell dump failed");
    }
}

pub(super) fn end_cell_dump(out: &mut impl Write) {
    if let Err(e) = execute!(out, EndSynchronizedUpdate) {
        tracing::debug!(error = %e, "end cell dump failed");
    }
}

pub(super) fn draw_into<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp,
) -> anyhow::Result<(ratatui::layout::Rect, Option<crate::cell_grid::CellGrid>)>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let completed = terminal
        .draw(|f| render::render(f, app))
        .map_err(anyhow::Error::from)?;
    let area = completed.area;
    let cells = if app.mouse_sel.is_some() {
        Some(crate::cell_grid::CellGrid::from_buffer(completed.buffer))
    } else {
        None
    };
    Ok((area, cells))
}

/// Writer for alt-screen / draw / mouse: controlling console first, else stdout if TTY.
pub(super) fn open_tui_writer() -> io::Result<TuiWriter> {
    choose_tui_writer(open_controlling_console(), io::stdout().is_terminal())
}

pub(super) fn choose_tui_writer(
    console: Option<std::fs::File>,
    stdout_is_tty: bool,
) -> io::Result<TuiWriter> {
    if let Some(console) = console {
        return Ok(TuiWriter::Console(io::BufWriter::with_capacity(
            CONSOLE_WRITE_BUFFER,
            console,
        )));
    }
    if stdout_is_tty {
        return Ok(TuiWriter::Stdout(io::stdout()));
    }
    Err(io::Error::new(
        io::ErrorKind::NotConnected,
        "no interactive terminal (stdout is not a TTY and the controlling console is unavailable)",
    ))
}

/// Open flags for `/dev/tty` (Unix). Always compiled so tests can drive the
/// builder without a Unix TTY.
#[cfg_attr(not(any(unix, test)), allow(dead_code))]
pub(super) fn unix_tty_open_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true);
    o
}

/// Open flags for `CONOUT$` (Windows). Always compiled so tests can drive the
/// builder without a live console.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) fn windows_console_open_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    o
}

/// Open `/dev/tty` with Unix flags. Always compiled so tests can drive the
/// builder + path without a Unix host.
#[cfg_attr(not(any(unix, test)), allow(dead_code))]
pub(super) fn try_open_unix_tty() -> io::Result<std::fs::File> {
    unix_tty_open_options().open("/dev/tty")
}

/// Open `CONOUT$` with Windows flags. Always compiled so tests can drive it
/// without a live console.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) fn try_open_windows_console() -> io::Result<std::fs::File> {
    windows_console_open_options().open("CONOUT$")
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
pub(super) fn open_unix_controlling_console() -> Option<std::fs::File> {
    console_open_result(try_open_unix_tty(), "open /dev/tty failed, trying stdout")
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) fn open_windows_controlling_console() -> Option<std::fs::File> {
    console_open_result(
        try_open_windows_console(),
        "open CONOUT$ failed, trying stdout",
    )
}

/// `/dev/tty` on Unix, `CONOUT$` on Windows. `None` if this process has no console.
pub(super) fn open_controlling_console() -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        open_unix_controlling_console()
    }
    #[cfg(windows)]
    {
        open_windows_controlling_console()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

pub(super) fn console_open_result(
    result: io::Result<std::fs::File>,
    msg: &'static str,
) -> Option<std::fs::File> {
    match result {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::debug!(error = %e, "{msg}");
            None
        }
    }
}

/// Leave alt-screen / raw mode from the panic hook (and from tests).
pub(super) fn restore_terminal_on_panic() {
    restore_terminal_with(open_tui_writer)
}

pub(super) fn restore_terminal_with(open: impl FnOnce() -> io::Result<TuiWriter>) {
    if let Ok(mut out) = open() {
        restore_terminal_on(&mut out);
    } else {
        let _ = disable_raw_mode();
    }
}

pub(super) fn install_panic_terminal_restore() {
    whycodes_core::logging::set_panic_cleanup(restore_terminal_on_panic);
}

pub(super) fn restore_terminal_on(out: &mut impl Write) {
    let _ = disable_raw_mode();
    disable_focus_reports(out);
    let _ = execute!(
        out,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        // Prompt used a blinking bar; give the shell the user's shape back.
        SetCursorStyle::DefaultUserShape,
        crossterm::cursor::Show
    );
    pop_title(out);
    windows_console::restore_windows_console();
}

/// Probe + enable the kitty keyboard protocol.
///
/// Returns whether flags were pushed (so shutdown can pop them). A 0×0 PTY
/// or `WHYCODES_BENCH` run never answers the CSI query; skip it rather than
/// stalling the first paint for crossterm's ~2 s timeout.
pub(super) fn enable_keyboard_enhancement(out: &mut impl Write, size: Option<(u16, u16)>) -> bool {
    if !should_query_keyboard_enhancement(
        std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty()),
        size,
    ) {
        return false;
    }
    push_keyboard_flags(out, keyboard_enhancement_supported())
}

pub(super) fn keyboard_enhancement_supported() -> bool {
    matches!(supports_keyboard_enhancement(), Ok(true))
}

pub(super) fn push_keyboard_flags(out: &mut impl Write, supported: bool) -> bool {
    if !supported {
        return false;
    }
    execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

pub(super) fn attach_live(
    color_mode: ColorMode,
    open: impl FnOnce() -> io::Result<TuiWriter>,
    enable_raw: impl FnOnce() -> io::Result<()>,
    size: impl FnOnce() -> io::Result<(u16, u16)>,
) -> anyhow::Result<(LoopTerm, bool, u16, u16)> {
    let mut tui_out = open().map_err(|e| {
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.open_writer_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!(
            "failed to open terminal for TUI ({e}). \
             Run inside a real terminal, or use `whycodes --plain`."
        )
    })?;
    // `CONOUT$` is a new handle: crossterm's stdout VT flag does not apply,
    // and the OEM code page turns UTF-8 box drawing into `Ööö` on TR Windows.
    windows_console::prepare_windows_console(&tui_out);
    if let Err(e) = enter_raw_and_alt(&mut tui_out, enable_raw) {
        windows_console::restore_windows_console();
        return Err(e);
    }
    let (tw, th) = size().unwrap_or((0, 0));
    let keyboard_enhanced = enable_keyboard_enhancement(&mut tui_out, Some((tw, th)));
    let mut terminal = LoopTerm::live(tui_out, color_mode)?;
    if tw == 0 || th == 0 {
        terminal.resize(Rect::new(0, 0, 80, 24));
        whycodes_core::logging::emit(
            "whycodes_tui",
            "warn",
            "tui.size_fallback",
            Some(serde_json::json!({ "reported_w": tw, "reported_h": th, "using": "80x24" })),
        );
    }
    Ok((terminal, keyboard_enhanced, tw, th))
}

pub(super) fn enter_raw_and_alt(
    out: &mut impl Write,
    enable_raw: impl FnOnce() -> io::Result<()>,
) -> anyhow::Result<()> {
    enable_raw().map_err(|e| {
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.raw_mode_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!(
            "failed to enter raw mode ({e}). \
             Run inside a real terminal, or use `whycodes --plain`."
        )
    })?;
    execute!(out, EnterAlternateScreen, Print(TITLE_PUSH), Hide).map_err(|e| {
        let _ = disable_raw_mode();
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.alt_screen_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!("failed to enter alternate screen ({e})")
    })?;
    Ok(())
}

pub(super) fn enable_mouse_paste_cursor(out: &mut impl Write) {
    if let Err(e) = execute!(
        out,
        EnableMouseCapture,
        EnableBracketedPaste,
        SetCursorStyle::BlinkingBar,
        // Shape only — keep hidden until the next draw places the bar
        // on the prompt. Showing it here lets Windows walk the first
        // home-screen dump (splash → themed prompt).
        Hide
    ) {
        tracing::debug!(error = %e, "enable mouse/paste/cursor after first paint failed");
    }
    if crate::desktop_notify::wants_focus_reports()
        && let Err(e) = execute!(out, EnableFocusChange)
    {
        tracing::debug!(error = %e, "enable focus reports failed");
    }
}

/// Undo [`enable_mouse_paste_cursor`]'s focus reports (desktop notify only).
pub(super) fn disable_focus_reports(out: &mut impl Write) {
    if crate::desktop_notify::wants_focus_reports()
        && let Err(e) = execute!(out, DisableFocusChange)
    {
        tracing::debug!(error = %e, "disable focus reports failed");
    }
}

pub(super) fn restore_live_backend(out: &mut impl Write, keyboard_enhanced: bool) {
    let _ = disable_raw_mode();
    disable_focus_reports(out);
    if keyboard_enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        SetCursorStyle::DefaultUserShape
    );
    pop_title(out);
    windows_console::restore_windows_console();
}

/// Whether it is worth waiting on the keyboard-enhancement CSI query.
///
/// Pure so the 0×0 / bench skip can be unit-tested without a real PTY.
pub(super) fn should_query_keyboard_enhancement(bench: bool, size: Option<(u16, u16)>) -> bool {
    if bench {
        return false;
    }
    matches!(size, Some((w, h)) if w > 0 && h > 0)
}

/// One CSI burst: alt-screen, clear, product line. No ratatui, no raw mode.
pub(super) fn write_splash_csi(out: &mut impl Write) -> io::Result<()> {
    out.write_all(b"\x1b[?1049h\x1b[2J\x1b[H\x1b[?25lwhycodes  -  /help")?;
    out.flush()
}

pub(super) fn restore_splash_csi(out: &mut impl Write) {
    if let Err(e) = out.write_all(b"\x1b[?25h\x1b[?1049l") {
        tracing::debug!(error = %e, "splash CSI restore write failed");
    }
    if let Err(e) = out.flush() {
        tracing::debug!(error = %e, "splash CSI restore flush failed");
    }
    windows_console::restore_windows_console();
}

pub(super) fn render_splash(frame: &mut ratatui::Frame<'_>) {
    use ratatui::style::{Color, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Paragraph};
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );
    let line = Paragraph::new(Line::from("whycodes  —  /help"));
    frame.render_widget(line, area);
}

/// Real crossterm poll. Tests call this with `Duration::ZERO` so the
/// production line runs without a TTY wait.
pub(super) fn production_term_size() -> io::Result<(u16, u16)> {
    term_size().or(Ok((0, 0)))
}

pub(super) fn live_buf_open() -> io::Result<TuiWriter> {
    Ok(TuiWriter::Buf(Vec::new()))
}

pub(super) fn live_buf_raw() -> io::Result<()> {
    Ok(())
}

pub(super) fn live_buf_size() -> io::Result<(u16, u16)> {
    Ok((0, 0))
}

pub(super) type OpenTui = fn() -> io::Result<TuiWriter>;

pub(super) type EnableRaw = fn() -> io::Result<()>;

pub(super) type TermSize = fn() -> io::Result<(u16, u16)>;

/// Live-buffer tests swap in memory writers; production uses the controlling
/// console, raw mode, and the real terminal size.
pub(super) fn loop_attach_io(live_buf: bool) -> (OpenTui, EnableRaw, TermSize) {
    if live_buf {
        (live_buf_open, live_buf_raw, live_buf_size)
    } else {
        (open_tui_writer, enable_raw_mode, production_term_size)
    }
}

pub(super) fn attach_for_loop(
    color_mode: ColorMode,
    live_buf: bool,
) -> anyhow::Result<(LoopTerm, bool, u16, u16)> {
    let (open, raw, size) = loop_attach_io(live_buf);
    attach_live(color_mode, open, raw, size)
}

pub(super) fn live_poll_crossterm(timeout: Duration) -> io::Result<bool> {
    poll_crossterm_with(timeout, crossterm::event::poll)
}

/// Injected poll so tests can drive the live-poll wrapper without a TTY.
pub(super) fn poll_crossterm_with(
    timeout: Duration,
    poll: impl FnOnce(Duration) -> io::Result<bool>,
) -> io::Result<bool> {
    poll(timeout)
}

/// Real crossterm read. Tests only call this after a zero-timeout poll
/// returns true (otherwise it would block on a missing TTY).
pub(super) fn live_read_crossterm() -> io::Result<Event> {
    read_crossterm_with(crossterm::event::read)
}

pub(super) fn read_crossterm_with(read: impl FnOnce() -> io::Result<Event>) -> io::Result<Event> {
    read()
}

/// Tests force a zero timeout so an empty stub cannot block on a missing TTY.
#[cfg(test)]
pub(super) fn poll_timeout(requested: Duration, force_zero: bool) -> Duration {
    if force_zero {
        Duration::ZERO
    } else {
        requested
    }
}
