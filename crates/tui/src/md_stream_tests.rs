use super::*;
use crate::config::TuiAppConfig;

fn palette() -> ThemePalette {
    TuiAppConfig::default().palette()
}

fn line_text(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn incremental_matches_full_render_across_checkpoints() {
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let width = Some(40usize);
    let mut acc = String::new();
    for chunk in [
        "Here is **setup**.\n\n",
        "```rust\nfn main() {\n",
        "    println!(\"hi\");\n",
        "}\n```\n\n",
        "Done.\n",
    ] {
        acc.push_str(chunk);
        let got = inc.render(&acc, &palette, width);
        let full = render_with_width(&acc, &palette, width);
        assert_eq!(line_text(got), line_text(&full), "mismatch after {acc:?}");
    }
    assert!(
        inc.frozen_bytes > 0,
        "closed fence + blank must freeze a prefix"
    );
}

#[test]
fn open_line_does_not_freeze() {
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let _ = inc.render("partial", &palette, Some(40));
    assert_eq!(inc.frozen_bytes, 0);
}

#[test]
fn growing_open_fence_matches_full_render() {
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let width = Some(60usize);
    let mut acc = String::from("Intro.\n\n```rust\n");
    let got = inc.render(&acc, &palette, width);
    assert_eq!(
        line_text(got),
        line_text(&render_with_width(&acc, &palette, width))
    );
    for line in [
        "fn main() {\n",
        "    let x = 1;\n",
        "    let y = 2;\n",
        "    println!(\"{x}{y}\");\n",
        "}\n",
    ] {
        acc.push_str(line);
        let got = inc.render(&acc, &palette, width);
        let full = render_with_width(&acc, &palette, width);
        assert_eq!(line_text(got), line_text(&full), "mismatch after {acc:?}");
    }
    assert!(inc.fence_src > 0, "complete fence lines must commit");
    acc.push_str("```\n\nDone.\n");
    let got = inc.render(&acc, &palette, width);
    let full = render_with_width(&acc, &palette, width);
    assert_eq!(line_text(got), line_text(&full), "mismatch after close");
}

#[test]
fn open_fence_gutter_width_change_rebuilds_committed_rows() {
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let width = Some(60usize);
    let mut acc = String::from("Intro.\n\n```rust\n1\n");
    let _ = inc.render(&acc, &palette, width);
    assert!(inc.fence_src > 0);
    acc.push_str("10\n100\n");
    let got = inc.render(&acc, &palette, width);
    let full = render_with_width(&acc, &palette, width);
    assert_eq!(line_text(got), line_text(&full));
}
