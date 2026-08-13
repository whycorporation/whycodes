//! Load real opencode theme files.
//!
//! The unit tests in `theme/file.rs` use a hand-written sample, which only
//! proves the loader agrees with itself. These use files taken unmodified from
//! opencode — see `fixtures/NOTICE.md` — so they test the loader against the
//! schema as it is actually written rather than as we imagined it.

use ratatui::style::Color;
use std::path::PathBuf;
use whycode_tui::theme_file::{ThemeFile, load_dir};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixtures().join(name))
        .unwrap_or_else(|e| panic!("missing fixture {name}: {e}"))
}

#[test]
fn an_unmodified_opencode_theme_loads() {
    for name in ["opencode.json", "tokyonight.json"] {
        let file = ThemeFile::parse(&read(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        for light in [false, true] {
            file.palette(light)
                .unwrap_or_else(|e| panic!("{name} (light={light}): {e}"));
        }
    }
}

#[test]
fn resolved_colours_match_the_file() {
    // opencode.json defines darkStep1 = #0a0a0a and maps background.dark to it.
    let file = ThemeFile::parse(&read("opencode.json")).unwrap();
    assert_eq!(
        file.palette(false).bg_or_panic(),
        Color::Rgb(0x0a, 0x0a, 0x0a)
    );
    // …and lightStep1 = #ffffff for the light variant.
    assert_eq!(
        file.palette(true).bg_or_panic(),
        Color::Rgb(0xff, 0xff, 0xff)
    );
}

#[test]
fn the_variants_are_actually_different() {
    let file = ThemeFile::parse(&read("tokyonight.json")).unwrap();
    let dark = file.palette(false).unwrap();
    let light = file.palette(true).unwrap();
    assert_ne!(
        dark.bg, light.bg,
        "a light variant that matches the dark one means the variant is being ignored"
    );
    assert_ne!(dark.fg, light.fg);
}

#[test]
fn loading_the_fixture_directory_yields_both_variants_of_each_file() {
    let (loaded, errors) = load_dir(&fixtures());
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");

    let mut names: Vec<&str> = loaded.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "opencode",
            "opencode-light",
            "tokyonight",
            "tokyonight-light"
        ]
    );
}

#[test]
fn the_notice_file_is_not_mistaken_for_a_theme() {
    // `NOTICE.md` sits in the same directory; only `*.json` should be read.
    let (loaded, _) = load_dir(&fixtures());
    assert!(!loaded.iter().any(|t| t.name.contains("NOTICE")));
}

/// Small helper so the assertions above read as one line.
trait PaletteExt {
    fn bg_or_panic(self) -> Color;
}

impl PaletteExt
    for Result<whycode_tui::theme::ThemePalette, whycode_tui::theme_file::ThemeFileError>
{
    fn bg_or_panic(self) -> Color {
        self.expect("palette should resolve").bg
    }
}
