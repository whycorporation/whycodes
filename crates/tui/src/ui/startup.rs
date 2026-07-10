// ── startup.rs: Startup loading screen (like OpenCode's startup-loading.tsx) ─
// Shows version, provider info, project path with a loading spinner.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Spinner frames for the loading animation.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Startup screen state.
pub struct StartupScreen {
    /// Progress from 0.0 to 1.0.
    pub progress: f64,
    /// Frame index for the spinner animation.
    pub spinner_frame: usize,
    /// Duration in milliseconds this screen should be shown.
    pub duration_ms: u64,
    /// Elapsed milliseconds.
    pub elapsed_ms: u64,
    /// Whether startup is complete (fade out).
    pub done: bool,
    /// Current version string.
    pub version: String,
    /// Provider info display string.
    pub provider_info: String,
    /// Project path.
    pub project_path: String,
    /// Sub-status message.
    pub status: String,
}

impl StartupScreen {
    pub fn new(duration_ms: u64) -> Self {
        Self {
            progress: 0.0,
            spinner_frame: 0,
            duration_ms,
            elapsed_ms: 0,
            done: false,
            version: String::from("0.1.0"),
            provider_info: String::from("Loading..."),
            project_path: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| String::from(".")),
            status: String::from("Initializing..."),
        }
    }

    /// Advance the animation by `delta_ms` milliseconds.
    /// Returns true if startup should now transition to the main app.
    pub fn tick(&mut self, delta_ms: u64) -> bool {
        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);
        self.progress = (self.elapsed_ms as f64 / self.duration_ms as f64).min(1.0);
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();

        // Update status messages based on progress
        if self.progress < 0.25 {
            self.status = String::from("Loading configuration...");
        } else if self.progress < 0.5 {
            self.status = String::from("Connecting to providers...");
        } else if self.progress < 0.75 {
            self.status = String::from("Preparing workspace...");
        } else if self.progress < 0.95 {
            self.status = String::from("Almost ready...");
        } else {
            self.status = String::from("Ready!");
        }

        if self.elapsed_ms >= self.duration_ms {
            self.done = true;
            true
        } else {
            false
        }
    }

    /// Get the current spinner character.
    pub fn spinner(&self) -> &str {
        SPINNER_FRAMES[self.spinner_frame]
    }
}

/// Render the startup screen. At `opacity=1.0` it's fully visible; at 0.0, invisible.
pub fn render(frame: &mut Frame, area: Rect, screen: &StartupScreen) {
    // Calculate alpha from elapsed vs duration
    let fade_alpha: f64 = if screen.progress < 0.8 {
        1.0
    } else {
        // Fade out in the last 20% of time
        1.0 - ((screen.progress - 0.8) / 0.2)
    };

    // Apply alpha to colors by blending with background
    let blend = |base: (u8, u8, u8), alpha: f64| -> Color {
        let bg = (18u8, 18u8, 24u8);
        let r = (base.0 as f64 * alpha + bg.0 as f64 * (1.0 - alpha)) as u8;
        let g = (base.1 as f64 * alpha + bg.1 as f64 * (1.0 - alpha)) as u8;
        let b = (base.2 as f64 * alpha + bg.2 as f64 * (1.0 - alpha)) as u8;
        Color::Rgb(r, g, b)
    };

    // Center the startup content
    let popup_area = crate::widgets::centered_rect(50, 40, area);
    frame.render_widget(Clear, popup_area);

    let accent = blend((100, 149, 237), fade_alpha);
    let fg = blend((220, 220, 220), fade_alpha);
    let muted = blend((120, 120, 140), fade_alpha);

    let version = env!("CARGO_PKG_VERSION");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  ╔══════════════════════════════════════╗",
            Style::default().fg(accent),
        )),
        Line::from(vec![
            Span::styled("  ║", Style::default().fg(accent)),
            Span::styled(
                format!("       whycode v{}        ", version),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled("║", Style::default().fg(accent)),
        ]),
        Line::from(Span::styled(
            "  ╚══════════════════════════════════════╝",
            Style::default().fg(accent),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} {}", screen.spinner(), screen.status),
            Style::default().fg(muted),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Provider : {}", screen.provider_info),
            Style::default().fg(muted),
        )),
        Line::from(Span::styled(
            format!("  Project  : {}", screen.project_path),
            Style::default().fg(muted),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  [{}{}]",
                "█".repeat((screen.progress * 20.0) as usize),
                "░".repeat((20.0 - screen.progress * 20.0).max(0.0) as usize),
            ),
            Style::default().fg(accent),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press ? for help  |  : for commands",
            Style::default().fg(muted),
        )),
    ];

    let block = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true });

    frame.render_widget(block, popup_area);
}
