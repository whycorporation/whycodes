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

/// A reply with every checkpoint shape: prose, lists, closed and open fences,
/// and a table that only becomes one once its separator row arrives.
const MIXED_REPLY: &str = "Intro with **bold** text.\n\n- one\n- two\n\n\
```rust\nfn main() {\n    let x = 1;\n}\n```\n\n\
| a | b |\n|---|---|\n| 1 | 2 |\n\nAfter the table.\n\n\
```\nplain fence\n```\nTail line that keeps growing";

#[test]
fn alternating_widths_stay_incremental_and_match_full_render() {
    // An overflowing transcript measures the live bubble at two widths every
    // frame. Each width must keep its own frozen prefix instead of resetting.
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let (wide, narrow) = (Some(80usize), Some(78usize));
    let mut acc = String::new();
    for chunk in MIXED_REPLY.as_bytes().chunks(7) {
        acc.push_str(std::str::from_utf8(chunk).expect("ascii"));
        for width in [wide, narrow] {
            let got = inc.render(&acc, &palette, width);
            let full = render_with_width(&acc, &palette, width);
            assert_eq!(
                line_text(got),
                line_text(&full),
                "width {width:?} after {acc:?}"
            );
        }
    }
    // The wide slot is parked with its frozen prefix, not discarded.
    let parked = inc.other.as_ref().map(|o| (o.width, o.frozen_bytes > 0));
    assert_eq!(parked, Some((wide, true)));
    let frozen_lines = inc.other.as_ref().map(|o| o.frozen_len).unwrap_or(0);
    let _ = inc.render(&acc, &palette, wide);
    assert_eq!(inc.width, wide);
    assert_eq!(
        inc.frozen_len, frozen_lines,
        "switching back must reuse the wide frozen prefix"
    );
}

#[test]
fn width_switch_after_replaced_text_rebuilds_the_parked_slot() {
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let first = "Old paragraph.\n\nMore old text.\n\n";
    let _ = inc.render(first, &palette, Some(60));
    let _ = inc.render(first, &palette, Some(58));
    let replaced = "New answer.\n\n";
    let _ = inc.render(replaced, &palette, Some(58));
    let got = inc.render(replaced, &palette, Some(60));
    let full = render_with_width(replaced, &palette, Some(60));
    assert_eq!(line_text(got), line_text(&full));
}

#[test]
fn checkpoint_from_frozen_prefix_matches_full_scan() {
    use whycodes_format::markdown::last_checkpoint;
    let mut inc = IncrementalMarkdown::default();
    let palette = palette();
    let mut acc = String::new();
    for chunk in MIXED_REPLY.as_bytes().chunks(5) {
        acc.push_str(std::str::from_utf8(chunk).expect("ascii"));
        let _ = inc.render(&acc, &palette, Some(70));
        assert_eq!(inc.frozen_bytes, last_checkpoint(&acc), "after {acc:?}");
    }
}
