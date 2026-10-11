//! Streaming frame cost: paint the full TUI after every token batch of a long
//! markdown reply and report per-frame time as the reply grows.
//!
//! Ignored by default (timing, not correctness). Run in release:
//!
//! ```text
//! cargo test -p whycodes-tui --release --lib stream_frame_bench -- --ignored --nocapture
//! ```
//!
//! A frame cost that grows with reply length means the live bubble is being
//! re-parsed instead of extended (see `md_stream.rs`).

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{AgentState, ChatRole, TuiApp};
use crate::config::TuiAppConfig;

/// Bytes appended per frame — roughly one coalesced batch of stream deltas.
const CHUNK: usize = 48;

fn long_reply(sections: usize) -> String {
    let mut s = String::new();
    for i in 0..sections {
        s.push_str(&format!("## Section {i}\n\n"));
        s.push_str(
            "Streaming replies mix prose with **bold**, `inline code` and \
             [links](https://example.com) that wrap across several rows at \
             normal terminal widths.\n\n",
        );
        s.push_str("- first point\n- second point with more words\n- third\n\n");
        s.push_str("```rust\n");
        for j in 0..12 {
            s.push_str(&format!(
                "let value_{j} = compute({i}, {j}).unwrap_or_default();\n"
            ));
        }
        s.push_str("```\n\n");
    }
    s
}

fn history(app: &mut TuiApp, pairs: usize) {
    for i in 0..pairs {
        app.add_message(ChatRole::User, format!("question {i} with a few words"));
        app.add_message(
            ChatRole::Assistant,
            format!("answer {i}\n\n{}", long_reply(1)),
        );
    }
}

#[test]
#[ignore = "timing harness; run with --ignored --nocapture in release"]
fn stream_frame_bench() {
    let reply = long_reply(60);
    let mut app = TuiApp::new(TuiAppConfig::default());
    history(&mut app, 20);
    app.add_message(ChatRole::User, "write the long answer");
    app.add_message(ChatRole::Assistant, "");
    app.current_agent_state = AgentState::Generating;

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    let mut frames: Vec<Duration> = Vec::new();
    let mut at = 0;
    while at < reply.len() {
        let mut end = (at + CHUNK).min(reply.len());
        while !reply.is_char_boundary(end) {
            end += 1;
        }
        app.append_to_last(&reply[at..end]);
        at = end;
        let t = Instant::now();
        terminal
            .draw(|f| super::render::render(f, &mut app))
            .expect("draw");
        frames.push(t.elapsed());
    }

    let quarter = frames.len() / 4;
    println!(
        "stream_frame_bench: {} bytes, {} frames, 120x40",
        reply.len(),
        frames.len()
    );
    for (q, part) in frames.chunks(quarter.max(1)).take(4).enumerate() {
        let mut sorted: Vec<Duration> = part.to_vec();
        sorted.sort();
        let mean = part.iter().sum::<Duration>() / part.len() as u32;
        let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
        println!(
            "  quarter {}: mean {:.3} ms  p95 {:.3} ms",
            q + 1,
            mean.as_secs_f64() * 1e3,
            p95.as_secs_f64() * 1e3
        );
    }
    let total: Duration = frames.iter().sum();
    println!("  total paint {:.1} ms", total.as_secs_f64() * 1e3);

    // Same transcript once the turn ends: every bubble is cached, so an
    // unchanged frame should cost about as much as an empty one.
    app.current_agent_state = AgentState::Idle;
    let n = 50u32;
    let t = Instant::now();
    for _ in 0..n {
        terminal
            .draw(|f| super::render::render(f, &mut app))
            .expect("draw");
    }
    println!(
        "  idle frame {:.3} ms",
        t.elapsed().as_secs_f64() * 1e3 / f64::from(n)
    );
}
