//! Frames for the README's animated terminal (`assets/tui-demo.svg`).
//!
//! Paints a scripted turn through the real renderer — home screen, typed
//! prompt, tool calls, streamed reply — and writes every cell of every frame
//! as JSON. `scripts/render_readme_demo.py` runs this and turns the dump into
//! the SVG, so the picture changes when the TUI does.
//!
//! ```text
//! WHYCODES_DEMO_FRAMES=frames.json cargo test -p whycodes-tui --lib readme_demo_frames -- --ignored
//! ```

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use serde_json::json;

use crate::app::{AgentState, ChatRole, TuiApp};
use crate::config::TuiAppConfig;

const COLS: u16 = 96;
const ROWS: u16 = 30;
const PROMPT: &str = "Add a dark mode toggle to the settings page";

/// One step of the script: a TUI state shown for `hold_ms`.
struct Frame {
    hold_ms: u64,
    /// Prompt caret cell, when the frame shows one.
    caret: Option<(u16, u16)>,
    cells: Vec<Vec<serde_json::Value>>,
}

struct Demo {
    app: TuiApp,
    terminal: Terminal<TestBackend>,
    frames: Vec<Frame>,
    /// Scripted time since Enter; drives the turn timer.
    turn_ms: u64,
}

impl Demo {
    fn new() -> Self {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.provider_name = "anthropic".into();
        app.model_name = "claude-sonnet-5-5".into();
        app.agent_name = "build".into();
        app.project_dir = "/home/dev/my-project".into();
        app.project_label = "my-project".into();
        app.git_branch = Some("main".into());
        let terminal = Terminal::new(TestBackend::new(COLS, ROWS)).expect("test terminal");
        Self {
            app,
            terminal,
            frames: Vec::new(),
            turn_ms: 0,
        }
    }

    /// Paint the current state and keep it on screen for `hold_ms`.
    fn shot(&mut self, hold_ms: u64, caret: bool) {
        if self.app.is_busy() {
            self.app.turn_started_at =
                Instant::now().checked_sub(Duration::from_millis(self.turn_ms));
            self.turn_ms += hold_ms;
            self.app.spinner_frame = self.app.spinner_frame.wrapping_add(1);
        }
        for m in &mut self.app.messages {
            m.invalidate_layout();
        }
        let app = &mut self.app;
        self.terminal
            .draw(|f| super::render::render(f, app))
            .expect("draw");
        let caret = caret
            .then(|| self.terminal.get_cursor_position().ok())
            .flatten()
            .map(|p| (p.x, p.y));
        let buf = self.terminal.backend().buffer();
        let cells = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        let c = &buf[(x, y)];
                        json!([
                            c.symbol(),
                            hex(c.fg),
                            hex(c.bg),
                            c.modifier.contains(Modifier::BOLD),
                        ])
                    })
                    .collect()
            })
            .collect();
        self.frames.push(Frame {
            hold_ms,
            caret,
            cells,
        });
    }

    fn assistant(&mut self) -> &mut crate::app::ChatMessage {
        self.app.messages.last_mut().expect("assistant message")
    }

    fn tool(&mut self, id: &str, name: &str, input: serde_json::Value, result: &str, ms: u64) {
        self.app.status_message = format!("Running tool `{name}`…");
        self.app.add_tool_call(id.into(), name.into(), input);
        self.shot(ms, false);
        self.app.add_tool_result(id, result, false);
        self.app.status_message.clear();
    }
}

fn hex(c: Color) -> serde_json::Value {
    let rgb = match c {
        Color::Reset => return serde_json::Value::Null,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Gray => (192, 192, 192),
        Color::DarkGray => (128, 128, 128),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        other => panic!("demo palette uses an unmapped color: {other:?}"),
    };
    let mut s = String::from("#");
    for v in [rgb.0, rgb.1, rgb.2] {
        let _ = write!(s, "{v:02x}");
    }
    json!(s)
}

const DIFF: &str = "\
--- a/src/pages/Settings.tsx
+++ b/src/pages/Settings.tsx
@@ -14,6 +14,11 @@ export function Settings() {
   return (
     <Section title=\"Appearance\">
+      <Toggle
+        label=\"Dark mode\"
+        checked={theme === \"dark\"}
+        onChange={(on) => setTheme(on ? \"dark\" : \"light\")}
+      />
       <FontSizePicker />";

// No inline `code`: a row holding one is washed to the code background.
const REPLY: &str = "Added a **Dark mode** toggle to the settings page. It writes through \
the existing theme store, so the choice persists and applies app-wide. All 6 tests pass.";

#[test]
#[ignore = "README asset generator; run via scripts/render_readme_demo.py"]
fn readme_demo_frames() {
    let out = std::env::var("WHYCODES_DEMO_FRAMES").expect("WHYCODES_DEMO_FRAMES=<path>");
    let mut d = Demo::new();

    // Home, then the prompt typed a few characters per frame.
    d.shot(1200, true);
    let chars: Vec<char> = PROMPT.chars().collect();
    for end in (2..=chars.len()).step_by(2) {
        d.app.input_buffer = chars[..end].iter().collect();
        d.app.input_cursor = d.app.input_buffer.len();
        d.shot(45, true);
    }
    d.app.input_buffer = PROMPT.into();
    d.app.input_cursor = PROMPT.len();
    d.shot(400, true);

    // Enter: the turn starts.
    d.app.input_buffer.clear();
    d.app.input_cursor = 0;
    d.app.add_message(ChatRole::User, PROMPT);
    d.app.add_message(ChatRole::Assistant, "");
    d.app.current_agent_state = AgentState::Generating;
    d.shot(250, false);

    d.tool(
        "t1",
        "grep",
        json!({"pattern": "useTheme", "path": "src"}),
        "src/theme/store.ts:8:export function useTheme() {\nsrc/App.tsx:3:import { useTheme } from \"./theme/store\";",
        220,
    );
    d.tool(
        "t2",
        "read",
        json!({"path": "src/pages/Settings.tsx"}),
        "     1\timport { Section } from \"../ui/Section\";\n     2\timport { FontSizePicker } from \"../ui/FontSizePicker\";",
        180,
    );
    d.tool(
        "t3",
        "edit",
        json!({"path": "src/pages/Settings.tsx"}),
        DIFF,
        300,
    );
    d.tool(
        "t4",
        "run",
        json!({"command": "npm test -- settings"}),
        "PASS src/pages/Settings.test.tsx\nTests: 6 passed, 6 total",
        650,
    );

    // Stream the reply a few words per frame.
    let words: Vec<&str> = REPLY.split_inclusive(' ').collect();
    for chunk in words.chunks(3) {
        d.app.append_to_last(&chunk.concat());
        d.shot(60, false);
    }

    // Done: the footer stamps the scripted turn time.
    d.app.current_agent_state = AgentState::Idle;
    let total = u128::from(d.turn_ms);
    d.assistant().duration_ms = Some(total);
    d.app.turn_started_at = None;
    d.app.context_used = 18_400;
    d.shot(3800, true);

    let frames: Vec<serde_json::Value> = d
        .frames
        .iter()
        .map(|f| json!({"hold_ms": f.hold_ms, "caret": f.caret, "cells": f.cells}))
        .collect();
    let doc = json!({"cols": COLS, "rows": ROWS, "frames": frames});
    std::fs::write(&out, doc.to_string()).expect("write frames");
}
