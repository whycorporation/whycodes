// ── ui/prompt.rs: OpenCode prompt chrome ───────────────────────────────
// From component/prompt: agent-colored left edge, model meta row,
// ▀ bottom hairline, max-width centered on home.

use crate::app::{AgentState, AppMode, TuiApp};
use crate::opencode_tokens::layout as oc_layout;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    // Center prompt on home (empty messages) with max width like home.tsx
    let area = if app.messages.is_empty() {
        center_prompt_area(area)
    } else {
        // session: full width of parent (already side-padded by parent)
        area
    };

    let busy = !matches!(
        app.current_agent_state,
        AgentState::Idle | AgentState::Error(_)
    );

    // OpenCode prompt uses left border color = agent color (peach primary for build)
    let edge = palette.agent_color_by_index(app.agent_cycle_idx);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // input row
            Constraint::Length(1), // meta: agent · model
        ])
        .split(area);

    // Input row with left ┃ accent (SplitBorder style)
    let (glyph, body_spans) = match app.mode {
        AppMode::Command => (
            ":",
            if app.command.buffer.is_empty() {
                vec![Span::styled(
                    " command…".to_string(),
                    Style::default().fg(palette.dim),
                )]
            } else {
                vec![
                    Span::styled(
                        format!(" {}", app.command.buffer),
                        Style::default().fg(palette.input_fg),
                    ),
                    Span::styled("▌".to_string(), Style::default().fg(palette.accent)),
                ]
            },
        ),
        _ if busy && app.input_buffer.is_empty() => (
            "…",
            vec![Span::styled(
                " generating…  esc interrupt".to_string(),
                Style::default().fg(palette.dim),
            )],
        ),
        _ if app.input_buffer.is_empty() => (
            ">",
            vec![Span::styled(
                " Fix a TODO in the codebase".to_string(),
                Style::default().fg(palette.dim),
            )],
        ),
        _ => (
            ">",
            vec![
                Span::styled(
                    format!(" {}", app.input_buffer),
                    Style::default().fg(palette.input_fg),
                ),
                Span::styled("▌".to_string(), Style::default().fg(palette.accent)),
            ],
        ),
    };

    let mut spans = vec![
        Span::styled("┃".to_string(), Style::default().fg(edge)),
        Span::styled(
            format!(" {glyph}"),
            Style::default()
                .fg(if busy { palette.dim } else { edge })
                .add_modifier(Modifier::BOLD),
        ),
    ];
    spans.extend(body_spans);

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.input_bg)),
        chunks[0],
    );

    // Meta row under prompt (OpenCode shows model · provider)
    let meta = Line::from(vec![
        Span::styled("┃".to_string(), Style::default().fg(edge)),
        Span::raw("  "),
        Span::styled(
            format!("{} ", app.agent_name),
            Style::default().fg(edge).add_modifier(Modifier::BOLD),
        ),
        Span::styled("· ".to_string(), Style::default().fg(palette.dim)),
        Span::styled(
            format!(
                "{} {}",
                if app.model_name.is_empty() {
                    "—".into()
                } else {
                    app.model_name.clone()
                },
                if app.provider_name.is_empty() {
                    String::new()
                } else {
                    app.provider_name.clone()
                }
            ),
            Style::default().fg(palette.dim),
        ),
    ]);

    // bottom hairline via ▀ like OpenCode EmptyBorder horizontal
    frame.render_widget(
        Paragraph::new(Text::from(meta)).style(Style::default().bg(palette.input_bg)),
        chunks[1],
    );

    let _ = Borders::ALL; // silence if unused in some builds
    let _ = Block::default();
}

fn center_prompt_area(area: Rect) -> Rect {
    let max_w = oc_layout::PROMPT_MAX_WIDTH
        .min((area.width as f32 * oc_layout::PROMPT_WIDTH_RATIO) as u16)
        .max(40)
        .min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(max_w)) / 2;
    Rect {
        x,
        y: area.y,
        width: max_w,
        height: area.height,
    }
}
