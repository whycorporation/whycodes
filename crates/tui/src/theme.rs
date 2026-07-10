// ── theme.rs: Color theme definitions ──────────────────────────────────
// Port of OpenCode's 29 themes — each theme provides a palette of
// semantic color roles used throughout the TUI.

use ratatui::style::Color;

// ── Theme Name ─────────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeName {
    DefaultDark,
    DefaultLight,
    Monokai,
    SolarizedDark,
    SolarizedLight,
    Nord,
    Dracula,
    Gruvbox,
    OneDark,
    CatppuccinMocha,
    CatppuccinLatte,
    TokyoNight,
    TokyoNightStorm,
    TokyoNightLight,
    Kanagawa,
    Everforest,
    RosePine,
    RosePineMoon,
    RosePineDawn,
    AyuDark,
    AyuMirage,
    AyuLight,
    GithubDark,
    GithubLight,
    VscodeDark,
    VscodeLight,
    Zenburn,
    OceanicNext,
    MaterialPalenight,
}

impl ThemeName {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "default_dark" | "default-dark" | "dark" => Self::DefaultDark,
            "default_light" | "default-light" | "light" => Self::DefaultLight,
            "monokai" => Self::Monokai,
            "solarized_dark" | "solarized-dark" => Self::SolarizedDark,
            "solarized_light" | "solarized-light" => Self::SolarizedLight,
            "nord" => Self::Nord,
            "dracula" => Self::Dracula,
            "gruvbox" => Self::Gruvbox,
            "one_dark" | "one-dark" | "onedark" => Self::OneDark,
            "catppuccin_mocha" | "catppuccin-mocha" => Self::CatppuccinMocha,
            "catppuccin_latte" | "catppuccin-latte" => Self::CatppuccinLatte,
            "tokyo_night" | "tokyo-night" | "tokyonight" => Self::TokyoNight,
            "tokyo_night_storm" | "tokyo-night-storm" | "tokyonightstorm" => Self::TokyoNightStorm,
            "tokyo_night_light" | "tokyo-night-light" | "tokyonightlight" => Self::TokyoNightLight,
            "kanagawa" => Self::Kanagawa,
            "everforest" => Self::Everforest,
            "rose_pine" | "rose-pine" | "rosepine" => Self::RosePine,
            "rose_pine_moon" | "rose-pine-moon" | "rosepinemoon" => Self::RosePineMoon,
            "rose_pine_dawn" | "rose-pine-dawn" | "rosepinedawn" => Self::RosePineDawn,
            "ayu_dark" | "ayu-dark" => Self::AyuDark,
            "ayu_mirage" | "ayu-mirage" => Self::AyuMirage,
            "ayu_light" | "ayu-light" => Self::AyuLight,
            "github_dark" | "github-dark" => Self::GithubDark,
            "github_light" | "github-light" => Self::GithubLight,
            "vscode_dark" | "vscode-dark" => Self::VscodeDark,
            "vscode_light" | "vscode-light" => Self::VscodeLight,
            "zenburn" => Self::Zenburn,
            "oceanic_next" | "oceanic-next" | "oceanicnext" => Self::OceanicNext,
            "material_palenight" | "material-palenight" | "palenight" => Self::MaterialPalenight,
            _ => Self::DefaultDark,
        }
    }

    pub fn palette(&self) -> ThemePalette {
        match self {
            Self::DefaultDark => palette_default_dark(),
            Self::DefaultLight => palette_default_light(),
            Self::Monokai => palette_monokai(),
            Self::SolarizedDark => palette_solarized_dark(),
            Self::SolarizedLight => palette_solarized_light(),
            Self::Nord => palette_nord(),
            Self::Dracula => palette_dracula(),
            Self::Gruvbox => palette_gruvbox(),
            Self::OneDark => palette_one_dark(),
            Self::CatppuccinMocha => palette_catppuccin_mocha(),
            Self::CatppuccinLatte => palette_catppuccin_latte(),
            Self::TokyoNight => palette_tokyo_night(),
            Self::TokyoNightStorm => palette_tokyo_night_storm(),
            Self::TokyoNightLight => palette_tokyo_night_light(),
            Self::Kanagawa => palette_kanagawa(),
            Self::Everforest => palette_everforest(),
            Self::RosePine => palette_rose_pine(),
            Self::RosePineMoon => palette_rose_pine_moon(),
            Self::RosePineDawn => palette_rose_pine_dawn(),
            Self::AyuDark => palette_ayu_dark(),
            Self::AyuMirage => palette_ayu_mirage(),
            Self::AyuLight => palette_ayu_light(),
            Self::GithubDark => palette_github_dark(),
            Self::GithubLight => palette_github_light(),
            Self::VscodeDark => palette_vscode_dark(),
            Self::VscodeLight => palette_vscode_light(),
            Self::Zenburn => palette_zenburn(),
            Self::OceanicNext => palette_oceanic_next(),
            Self::MaterialPalenight => palette_material_palenight(),
        }
    }
}

// ── Theme Palette ──────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct ThemePalette {
    pub bg: Color,
    pub fg: Color,
    pub border: Color,
    pub border_focused: Color,
    pub accent: Color,
    pub user_msg: Color,
    pub assistant_msg: Color,
    pub system_msg: Color,
    pub tool_msg: Color,
    pub thinking: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,
    pub dim: Color,
    pub highlight: Color,
    pub status_bar_bg: Color,
    pub status_bar_fg: Color,
    pub input_bg: Color,
    pub input_fg: Color,
    pub sidebar_bg: Color,
    pub dialog_bg: Color,
    pub dialog_border: Color,
    pub scrollbar: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub diff_hunk: Color,
}

// ── Palette factories ──────────────────────────────────────────────────
#[allow(clippy::too_many_lines)]
fn palette_default_dark() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(18, 18, 24),
        fg: Color::Rgb(212, 212, 218),
        border: Color::Rgb(60, 60, 70),
        border_focused: Color::Rgb(100, 140, 255),
        accent: Color::Rgb(100, 140, 255),
        user_msg: Color::Rgb(86, 182, 214),
        assistant_msg: Color::Rgb(125, 214, 140),
        system_msg: Color::Rgb(230, 200, 100),
        tool_msg: Color::Rgb(200, 160, 80),
        thinking: Color::Rgb(150, 120, 200),
        error: Color::Rgb(240, 80, 80),
        warning: Color::Rgb(240, 180, 60),
        success: Color::Rgb(100, 210, 100),
        info: Color::Rgb(100, 160, 220),
        dim: Color::Rgb(90, 90, 100),
        highlight: Color::Rgb(255, 255, 120),
        status_bar_bg: Color::Rgb(30, 30, 38),
        status_bar_fg: Color::Rgb(160, 160, 170),
        input_bg: Color::Rgb(22, 22, 30),
        input_fg: Color::Rgb(220, 220, 230),
        sidebar_bg: Color::Rgb(22, 22, 30),
        dialog_bg: Color::Rgb(28, 28, 38),
        dialog_border: Color::Rgb(100, 140, 255),
        scrollbar: Color::Rgb(80, 80, 90),
        diff_add: Color::Rgb(50, 160, 80),
        diff_remove: Color::Rgb(200, 60, 60),
        diff_hunk: Color::Rgb(100, 140, 255),
    }
}

fn palette_default_light() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(245, 245, 250),
        fg: Color::Rgb(30, 30, 40),
        border: Color::Rgb(200, 200, 210),
        border_focused: Color::Rgb(50, 90, 210),
        accent: Color::Rgb(50, 90, 210),
        user_msg: Color::Rgb(30, 120, 180),
        assistant_msg: Color::Rgb(30, 150, 80),
        system_msg: Color::Rgb(180, 140, 20),
        tool_msg: Color::Rgb(160, 100, 40),
        thinking: Color::Rgb(120, 80, 180),
        error: Color::Rgb(200, 40, 40),
        warning: Color::Rgb(200, 140, 20),
        success: Color::Rgb(40, 180, 80),
        info: Color::Rgb(40, 120, 200),
        dim: Color::Rgb(170, 170, 180),
        highlight: Color::Rgb(200, 160, 0),
        status_bar_bg: Color::Rgb(230, 230, 238),
        status_bar_fg: Color::Rgb(80, 80, 90),
        input_bg: Color::Rgb(235, 235, 242),
        input_fg: Color::Rgb(30, 30, 40),
        sidebar_bg: Color::Rgb(235, 235, 242),
        dialog_bg: Color::Rgb(240, 240, 245),
        dialog_border: Color::Rgb(50, 90, 210),
        scrollbar: Color::Rgb(190, 190, 200),
        diff_add: Color::Rgb(40, 180, 80),
        diff_remove: Color::Rgb(200, 40, 40),
        diff_hunk: Color::Rgb(50, 90, 210),
    }
}

// ── Remaining themes (abbreviated but all distinct) ────────────────────
fn palette_monokai() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(39, 40, 34), fg: Color::Rgb(248, 248, 242),
        border: Color::Rgb(73, 72, 62), border_focused: Color::Rgb(166, 226, 46),
        accent: Color::Rgb(166, 226, 46), user_msg: Color::Rgb(102, 217, 239),
        assistant_msg: Color::Rgb(166, 226, 46), system_msg: Color::Rgb(230, 219, 116),
        tool_msg: Color::Rgb(253, 151, 31), thinking: Color::Rgb(174, 129, 255),
        error: Color::Rgb(249, 38, 114), warning: Color::Rgb(230, 219, 116),
        success: Color::Rgb(166, 226, 46), info: Color::Rgb(102, 217, 239),
        dim: Color::Rgb(117, 113, 94), highlight: Color::Rgb(253, 151, 31),
        status_bar_bg: Color::Rgb(35, 36, 31), status_bar_fg: Color::Rgb(117, 113, 94),
        input_bg: Color::Rgb(35, 36, 31), input_fg: Color::Rgb(248, 248, 242),
        sidebar_bg: Color::Rgb(35, 36, 31), dialog_bg: Color::Rgb(46, 48, 40),
        dialog_border: Color::Rgb(166, 226, 46), scrollbar: Color::Rgb(73, 72, 62),
        diff_add: Color::Rgb(166, 226, 46), diff_remove: Color::Rgb(249, 38, 114),
        diff_hunk: Color::Rgb(174, 129, 255),
    }
}

fn palette_solarized_dark() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(0, 43, 54), fg: Color::Rgb(131, 148, 150),
        border: Color::Rgb(7, 54, 66), border_focused: Color::Rgb(38, 139, 210),
        accent: Color::Rgb(38, 139, 210), user_msg: Color::Rgb(38, 139, 210),
        assistant_msg: Color::Rgb(133, 153, 0), system_msg: Color::Rgb(181, 137, 0),
        tool_msg: Color::Rgb(203, 75, 22), thinking: Color::Rgb(108, 113, 196),
        error: Color::Rgb(220, 50, 47), warning: Color::Rgb(181, 137, 0),
        success: Color::Rgb(133, 153, 0), info: Color::Rgb(38, 139, 210),
        dim: Color::Rgb(88, 110, 117), highlight: Color::Rgb(203, 75, 22),
        status_bar_bg: Color::Rgb(0, 37, 46), status_bar_fg: Color::Rgb(88, 110, 117),
        input_bg: Color::Rgb(0, 37, 46), input_fg: Color::Rgb(131, 148, 150),
        sidebar_bg: Color::Rgb(0, 37, 46), dialog_bg: Color::Rgb(7, 54, 66),
        dialog_border: Color::Rgb(38, 139, 210), scrollbar: Color::Rgb(7, 54, 66),
        diff_add: Color::Rgb(133, 153, 0), diff_remove: Color::Rgb(220, 50, 47),
        diff_hunk: Color::Rgb(38, 139, 210),
    }
}

fn palette_solarized_light() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(253, 246, 227), fg: Color::Rgb(101, 123, 131),
        border: Color::Rgb(238, 232, 213), border_focused: Color::Rgb(38, 139, 210),
        accent: Color::Rgb(38, 139, 210), user_msg: Color::Rgb(38, 139, 210),
        assistant_msg: Color::Rgb(133, 153, 0), system_msg: Color::Rgb(181, 137, 0),
        tool_msg: Color::Rgb(203, 75, 22), thinking: Color::Rgb(108, 113, 196),
        error: Color::Rgb(220, 50, 47), warning: Color::Rgb(181, 137, 0),
        success: Color::Rgb(133, 153, 0), info: Color::Rgb(38, 139, 210),
        dim: Color::Rgb(147, 161, 161), highlight: Color::Rgb(203, 75, 22),
        status_bar_bg: Color::Rgb(238, 232, 213), status_bar_fg: Color::Rgb(147, 161, 161),
        input_bg: Color::Rgb(238, 232, 213), input_fg: Color::Rgb(101, 123, 131),
        sidebar_bg: Color::Rgb(238, 232, 213), dialog_bg: Color::Rgb(245, 242, 230),
        dialog_border: Color::Rgb(38, 139, 210), scrollbar: Color::Rgb(238, 232, 213),
        diff_add: Color::Rgb(133, 153, 0), diff_remove: Color::Rgb(220, 50, 47),
        diff_hunk: Color::Rgb(38, 139, 210),
    }
}

fn palette_nord() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(46, 52, 64), fg: Color::Rgb(216, 222, 233),
        border: Color::Rgb(59, 66, 82), border_focused: Color::Rgb(136, 192, 208),
        accent: Color::Rgb(136, 192, 208), user_msg: Color::Rgb(136, 192, 208),
        assistant_msg: Color::Rgb(163, 190, 140), system_msg: Color::Rgb(235, 203, 139),
        tool_msg: Color::Rgb(208, 135, 112), thinking: Color::Rgb(180, 142, 173),
        error: Color::Rgb(191, 97, 106), warning: Color::Rgb(235, 203, 139),
        success: Color::Rgb(163, 190, 140), info: Color::Rgb(136, 192, 208),
        dim: Color::Rgb(76, 86, 106), highlight: Color::Rgb(208, 135, 112),
        status_bar_bg: Color::Rgb(40, 44, 52), status_bar_fg: Color::Rgb(76, 86, 106),
        input_bg: Color::Rgb(40, 44, 52), input_fg: Color::Rgb(216, 222, 233),
        sidebar_bg: Color::Rgb(40, 44, 52), dialog_bg: Color::Rgb(54, 60, 72),
        dialog_border: Color::Rgb(136, 192, 208), scrollbar: Color::Rgb(59, 66, 82),
        diff_add: Color::Rgb(163, 190, 140), diff_remove: Color::Rgb(191, 97, 106),
        diff_hunk: Color::Rgb(136, 192, 208),
    }
}

fn palette_dracula() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(40, 42, 54), fg: Color::Rgb(248, 248, 242),
        border: Color::Rgb(68, 71, 90), border_focused: Color::Rgb(189, 147, 249),
        accent: Color::Rgb(189, 147, 249), user_msg: Color::Rgb(139, 233, 253),
        assistant_msg: Color::Rgb(80, 250, 123), system_msg: Color::Rgb(241, 250, 140),
        tool_msg: Color::Rgb(255, 184, 108), thinking: Color::Rgb(189, 147, 249),
        error: Color::Rgb(255, 85, 85), warning: Color::Rgb(241, 250, 140),
        success: Color::Rgb(80, 250, 123), info: Color::Rgb(139, 233, 253),
        dim: Color::Rgb(98, 114, 164), highlight: Color::Rgb(255, 184, 108),
        status_bar_bg: Color::Rgb(34, 36, 46), status_bar_fg: Color::Rgb(98, 114, 164),
        input_bg: Color::Rgb(34, 36, 46), input_fg: Color::Rgb(248, 248, 242),
        sidebar_bg: Color::Rgb(34, 36, 46), dialog_bg: Color::Rgb(50, 52, 66),
        dialog_border: Color::Rgb(189, 147, 249), scrollbar: Color::Rgb(68, 71, 90),
        diff_add: Color::Rgb(80, 250, 123), diff_remove: Color::Rgb(255, 85, 85),
        diff_hunk: Color::Rgb(189, 147, 249),
    }
}

fn palette_gruvbox() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(40, 40, 40), fg: Color::Rgb(235, 219, 178),
        border: Color::Rgb(60, 56, 54), border_focused: Color::Rgb(215, 153, 33),
        accent: Color::Rgb(250, 189, 47), user_msg: Color::Rgb(131, 165, 152),
        assistant_msg: Color::Rgb(184, 187, 38), system_msg: Color::Rgb(250, 189, 47),
        tool_msg: Color::Rgb(214, 93, 14), thinking: Color::Rgb(211, 134, 155),
        error: Color::Rgb(251, 73, 52), warning: Color::Rgb(250, 189, 47),
        success: Color::Rgb(184, 187, 38), info: Color::Rgb(131, 165, 152),
        dim: Color::Rgb(102, 92, 84), highlight: Color::Rgb(254, 128, 25),
        status_bar_bg: Color::Rgb(34, 34, 34), status_bar_fg: Color::Rgb(102, 92, 84),
        input_bg: Color::Rgb(34, 34, 34), input_fg: Color::Rgb(235, 219, 178),
        sidebar_bg: Color::Rgb(34, 34, 34), dialog_bg: Color::Rgb(50, 48, 46),
        dialog_border: Color::Rgb(250, 189, 47), scrollbar: Color::Rgb(60, 56, 54),
        diff_add: Color::Rgb(184, 187, 38), diff_remove: Color::Rgb(251, 73, 52),
        diff_hunk: Color::Rgb(250, 189, 47),
    }
}

fn palette_one_dark() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(40, 44, 52), fg: Color::Rgb(171, 178, 191),
        border: Color::Rgb(56, 60, 68), border_focused: Color::Rgb(97, 175, 239),
        accent: Color::Rgb(97, 175, 239), user_msg: Color::Rgb(97, 175, 239),
        assistant_msg: Color::Rgb(152, 195, 121), system_msg: Color::Rgb(229, 192, 123),
        tool_msg: Color::Rgb(209, 154, 102), thinking: Color::Rgb(198, 120, 221),
        error: Color::Rgb(224, 108, 117), warning: Color::Rgb(229, 192, 123),
        success: Color::Rgb(152, 195, 121), info: Color::Rgb(97, 175, 239),
        dim: Color::Rgb(92, 99, 112), highlight: Color::Rgb(229, 192, 123),
        status_bar_bg: Color::Rgb(34, 38, 44), status_bar_fg: Color::Rgb(92, 99, 112),
        input_bg: Color::Rgb(34, 38, 44), input_fg: Color::Rgb(171, 178, 191),
        sidebar_bg: Color::Rgb(34, 38, 44), dialog_bg: Color::Rgb(50, 54, 62),
        dialog_border: Color::Rgb(97, 175, 239), scrollbar: Color::Rgb(56, 60, 68),
        diff_add: Color::Rgb(152, 195, 121), diff_remove: Color::Rgb(224, 108, 117),
        diff_hunk: Color::Rgb(97, 175, 239),
    }
}

fn palette_catppuccin_mocha() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(30, 30, 46), fg: Color::Rgb(205, 214, 244),
        border: Color::Rgb(49, 50, 68), border_focused: Color::Rgb(203, 166, 247),
        accent: Color::Rgb(203, 166, 247), user_msg: Color::Rgb(137, 180, 250),
        assistant_msg: Color::Rgb(166, 227, 161), system_msg: Color::Rgb(249, 226, 175),
        tool_msg: Color::Rgb(250, 179, 135), thinking: Color::Rgb(203, 166, 247),
        error: Color::Rgb(243, 139, 168), warning: Color::Rgb(249, 226, 175),
        success: Color::Rgb(166, 227, 161), info: Color::Rgb(137, 180, 250),
        dim: Color::Rgb(88, 91, 112), highlight: Color::Rgb(250, 179, 135),
        status_bar_bg: Color::Rgb(24, 24, 37), status_bar_fg: Color::Rgb(88, 91, 112),
        input_bg: Color::Rgb(24, 24, 37), input_fg: Color::Rgb(205, 214, 244),
        sidebar_bg: Color::Rgb(24, 24, 37), dialog_bg: Color::Rgb(37, 37, 55),
        dialog_border: Color::Rgb(203, 166, 247), scrollbar: Color::Rgb(49, 50, 68),
        diff_add: Color::Rgb(166, 227, 161), diff_remove: Color::Rgb(243, 139, 168),
        diff_hunk: Color::Rgb(203, 166, 247),
    }
}

fn palette_catppuccin_latte() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(239, 241, 245), fg: Color::Rgb(76, 79, 105),
        border: Color::Rgb(204, 208, 218), border_focused: Color::Rgb(136, 57, 239),
        accent: Color::Rgb(136, 57, 239), user_msg: Color::Rgb(30, 102, 245),
        assistant_msg: Color::Rgb(64, 160, 43), system_msg: Color::Rgb(223, 142, 29),
        tool_msg: Color::Rgb(254, 100, 11), thinking: Color::Rgb(136, 57, 239),
        error: Color::Rgb(210, 15, 57), warning: Color::Rgb(223, 142, 29),
        success: Color::Rgb(64, 160, 43), info: Color::Rgb(30, 102, 245),
        dim: Color::Rgb(156, 160, 176), highlight: Color::Rgb(220, 138, 120),
        status_bar_bg: Color::Rgb(230, 233, 239), status_bar_fg: Color::Rgb(156, 160, 176),
        input_bg: Color::Rgb(230, 233, 239), input_fg: Color::Rgb(76, 79, 105),
        sidebar_bg: Color::Rgb(230, 233, 239), dialog_bg: Color::Rgb(242, 244, 248),
        dialog_border: Color::Rgb(136, 57, 239), scrollbar: Color::Rgb(204, 208, 218),
        diff_add: Color::Rgb(64, 160, 43), diff_remove: Color::Rgb(210, 15, 57),
        diff_hunk: Color::Rgb(136, 57, 239),
    }
}

fn palette_tokyo_night() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(26, 27, 38), fg: Color::Rgb(169, 177, 214),
        border: Color::Rgb(41, 46, 66), border_focused: Color::Rgb(122, 162, 247),
        accent: Color::Rgb(122, 162, 247), user_msg: Color::Rgb(122, 162, 247),
        assistant_msg: Color::Rgb(158, 206, 106), system_msg: Color::Rgb(224, 175, 104),
        tool_msg: Color::Rgb(255, 158, 100), thinking: Color::Rgb(187, 154, 247),
        error: Color::Rgb(247, 118, 142), warning: Color::Rgb(224, 175, 104),
        success: Color::Rgb(158, 206, 106), info: Color::Rgb(122, 162, 247),
        dim: Color::Rgb(86, 95, 137), highlight: Color::Rgb(224, 175, 104),
        status_bar_bg: Color::Rgb(22, 23, 32), status_bar_fg: Color::Rgb(86, 95, 137),
        input_bg: Color::Rgb(22, 23, 32), input_fg: Color::Rgb(169, 177, 214),
        sidebar_bg: Color::Rgb(22, 23, 32), dialog_bg: Color::Rgb(33, 35, 48),
        dialog_border: Color::Rgb(122, 162, 247), scrollbar: Color::Rgb(41, 46, 66),
        diff_add: Color::Rgb(158, 206, 106), diff_remove: Color::Rgb(247, 118, 142),
        diff_hunk: Color::Rgb(122, 162, 247),
    }
}

fn palette_tokyo_night_storm() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(36, 40, 59), fg: Color::Rgb(192, 202, 245),
        border: Color::Rgb(52, 57, 78), border_focused: Color::Rgb(125, 207, 255),
        accent: Color::Rgb(125, 207, 255), user_msg: Color::Rgb(125, 207, 255),
        assistant_msg: Color::Rgb(158, 206, 106), system_msg: Color::Rgb(224, 175, 104),
        tool_msg: Color::Rgb(255, 158, 100), thinking: Color::Rgb(187, 154, 247),
        error: Color::Rgb(247, 118, 142), warning: Color::Rgb(224, 175, 104),
        success: Color::Rgb(158, 206, 106), info: Color::Rgb(125, 207, 255),
        dim: Color::Rgb(86, 95, 137), highlight: Color::Rgb(255, 158, 100),
        status_bar_bg: Color::Rgb(30, 34, 50), status_bar_fg: Color::Rgb(86, 95, 137),
        input_bg: Color::Rgb(30, 34, 50), input_fg: Color::Rgb(192, 202, 245),
        sidebar_bg: Color::Rgb(30, 34, 50), dialog_bg: Color::Rgb(44, 48, 68),
        dialog_border: Color::Rgb(125, 207, 255), scrollbar: Color::Rgb(52, 57, 78),
        diff_add: Color::Rgb(158, 206, 106), diff_remove: Color::Rgb(247, 118, 142),
        diff_hunk: Color::Rgb(125, 207, 255),
    }
}

fn palette_tokyo_night_light() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(213, 219, 242), fg: Color::Rgb(52, 59, 88),
        border: Color::Rgb(192, 200, 230), border_focused: Color::Rgb(52, 84, 209),
        accent: Color::Rgb(52, 84, 209), user_msg: Color::Rgb(52, 84, 209),
        assistant_msg: Color::Rgb(72, 128, 38), system_msg: Color::Rgb(140, 94, 28),
        tool_msg: Color::Rgb(200, 80, 12), thinking: Color::Rgb(120, 62, 200),
        error: Color::Rgb(200, 42, 64), warning: Color::Rgb(140, 94, 28),
        success: Color::Rgb(72, 128, 38), info: Color::Rgb(52, 84, 209),
        dim: Color::Rgb(140, 148, 180), highlight: Color::Rgb(200, 80, 12),
        status_bar_bg: Color::Rgb(200, 206, 232), status_bar_fg: Color::Rgb(140, 148, 180),
        input_bg: Color::Rgb(200, 206, 232), input_fg: Color::Rgb(52, 59, 88),
        sidebar_bg: Color::Rgb(200, 206, 232), dialog_bg: Color::Rgb(224, 228, 248),
        dialog_border: Color::Rgb(52, 84, 209), scrollbar: Color::Rgb(192, 200, 230),
        diff_add: Color::Rgb(72, 128, 38), diff_remove: Color::Rgb(200, 42, 64),
        diff_hunk: Color::Rgb(52, 84, 209),
    }
}

fn palette_kanagawa() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(31, 31, 40), fg: Color::Rgb(220, 215, 186),
        border: Color::Rgb(54, 54, 62), border_focused: Color::Rgb(126, 156, 216),
        accent: Color::Rgb(126, 156, 216), user_msg: Color::Rgb(126, 156, 216),
        assistant_msg: Color::Rgb(152, 187, 108), system_msg: Color::Rgb(230, 200, 110),
        tool_msg: Color::Rgb(255, 160, 100), thinking: Color::Rgb(149, 127, 184),
        error: Color::Rgb(230, 90, 90), warning: Color::Rgb(230, 200, 110),
        success: Color::Rgb(152, 187, 108), info: Color::Rgb(126, 156, 216),
        dim: Color::Rgb(84, 84, 96), highlight: Color::Rgb(255, 160, 100),
        status_bar_bg: Color::Rgb(26, 26, 34), status_bar_fg: Color::Rgb(84, 84, 96),
        input_bg: Color::Rgb(26, 26, 34), input_fg: Color::Rgb(220, 215, 186),
        sidebar_bg: Color::Rgb(26, 26, 34), dialog_bg: Color::Rgb(40, 40, 50),
        dialog_border: Color::Rgb(126, 156, 216), scrollbar: Color::Rgb(54, 54, 62),
        diff_add: Color::Rgb(152, 187, 108), diff_remove: Color::Rgb(230, 90, 90),
        diff_hunk: Color::Rgb(126, 156, 216),
    }
}

fn palette_everforest() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(45, 53, 59), fg: Color::Rgb(211, 198, 170),
        border: Color::Rgb(61, 71, 78), border_focused: Color::Rgb(131, 192, 146),
        accent: Color::Rgb(131, 192, 146), user_msg: Color::Rgb(131, 192, 146),
        assistant_msg: Color::Rgb(167, 192, 128), system_msg: Color::Rgb(219, 188, 127),
        tool_msg: Color::Rgb(230, 152, 117), thinking: Color::Rgb(214, 153, 182),
        error: Color::Rgb(230, 126, 128), warning: Color::Rgb(219, 188, 127),
        success: Color::Rgb(167, 192, 128), info: Color::Rgb(131, 192, 146),
        dim: Color::Rgb(89, 102, 111), highlight: Color::Rgb(230, 152, 117),
        status_bar_bg: Color::Rgb(38, 45, 50), status_bar_fg: Color::Rgb(89, 102, 111),
        input_bg: Color::Rgb(38, 45, 50), input_fg: Color::Rgb(211, 198, 170),
        sidebar_bg: Color::Rgb(38, 45, 50), dialog_bg: Color::Rgb(53, 62, 68),
        dialog_border: Color::Rgb(131, 192, 146), scrollbar: Color::Rgb(61, 71, 78),
        diff_add: Color::Rgb(167, 192, 128), diff_remove: Color::Rgb(230, 126, 128),
        diff_hunk: Color::Rgb(131, 192, 146),
    }
}

fn palette_rose_pine() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(25, 23, 36), fg: Color::Rgb(224, 222, 244),
        border: Color::Rgb(38, 35, 58), border_focused: Color::Rgb(196, 167, 231),
        accent: Color::Rgb(196, 167, 231), user_msg: Color::Rgb(156, 207, 216),
        assistant_msg: Color::Rgb(144, 190, 126), system_msg: Color::Rgb(246, 193, 119),
        tool_msg: Color::Rgb(235, 188, 186), thinking: Color::Rgb(196, 167, 231),
        error: Color::Rgb(235, 111, 146), warning: Color::Rgb(246, 193, 119),
        success: Color::Rgb(144, 190, 126), info: Color::Rgb(156, 207, 216),
        dim: Color::Rgb(110, 106, 134), highlight: Color::Rgb(235, 188, 186),
        status_bar_bg: Color::Rgb(20, 18, 30), status_bar_fg: Color::Rgb(110, 106, 134),
        input_bg: Color::Rgb(20, 18, 30), input_fg: Color::Rgb(224, 222, 244),
        sidebar_bg: Color::Rgb(20, 18, 30), dialog_bg: Color::Rgb(33, 30, 46),
        dialog_border: Color::Rgb(196, 167, 231), scrollbar: Color::Rgb(38, 35, 58),
        diff_add: Color::Rgb(144, 190, 126), diff_remove: Color::Rgb(235, 111, 146),
        diff_hunk: Color::Rgb(196, 167, 231),
    }
}

fn palette_rose_pine_moon() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(35, 33, 54), fg: Color::Rgb(224, 222, 244),
        border: Color::Rgb(48, 45, 65), border_focused: Color::Rgb(196, 167, 231),
        accent: Color::Rgb(196, 167, 231), user_msg: Color::Rgb(156, 207, 216),
        assistant_msg: Color::Rgb(144, 190, 126), system_msg: Color::Rgb(246, 193, 119),
        tool_msg: Color::Rgb(235, 188, 186), thinking: Color::Rgb(196, 167, 231),
        error: Color::Rgb(235, 111, 146), warning: Color::Rgb(246, 193, 119),
        success: Color::Rgb(144, 190, 126), info: Color::Rgb(156, 207, 216),
        dim: Color::Rgb(110, 106, 134), highlight: Color::Rgb(235, 188, 186),
        status_bar_bg: Color::Rgb(28, 26, 44), status_bar_fg: Color::Rgb(110, 106, 134),
        input_bg: Color::Rgb(28, 26, 44), input_fg: Color::Rgb(224, 222, 244),
        sidebar_bg: Color::Rgb(28, 26, 44), dialog_bg: Color::Rgb(44, 42, 64),
        dialog_border: Color::Rgb(196, 167, 231), scrollbar: Color::Rgb(48, 45, 65),
        diff_add: Color::Rgb(144, 190, 126), diff_remove: Color::Rgb(235, 111, 146),
        diff_hunk: Color::Rgb(196, 167, 231),
    }
}

fn palette_rose_pine_dawn() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(250, 244, 237), fg: Color::Rgb(87, 82, 121),
        border: Color::Rgb(240, 222, 222), border_focused: Color::Rgb(144, 122, 169),
        accent: Color::Rgb(144, 122, 169), user_msg: Color::Rgb(40, 105, 131),
        assistant_msg: Color::Rgb(40, 120, 68), system_msg: Color::Rgb(190, 100, 40),
        tool_msg: Color::Rgb(180, 99, 122), thinking: Color::Rgb(144, 122, 169),
        error: Color::Rgb(180, 99, 122), warning: Color::Rgb(190, 100, 40),
        success: Color::Rgb(40, 120, 68), info: Color::Rgb(40, 105, 131),
        dim: Color::Rgb(155, 147, 175), highlight: Color::Rgb(210, 130, 80),
        status_bar_bg: Color::Rgb(242, 236, 230), status_bar_fg: Color::Rgb(155, 147, 175),
        input_bg: Color::Rgb(242, 236, 230), input_fg: Color::Rgb(87, 82, 121),
        sidebar_bg: Color::Rgb(242, 236, 230), dialog_bg: Color::Rgb(248, 245, 240),
        dialog_border: Color::Rgb(144, 122, 169), scrollbar: Color::Rgb(240, 222, 222),
        diff_add: Color::Rgb(40, 120, 68), diff_remove: Color::Rgb(180, 99, 122),
        diff_hunk: Color::Rgb(144, 122, 169),
    }
}

fn palette_ayu_dark() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(15, 20, 25), fg: Color::Rgb(191, 189, 171),
        border: Color::Rgb(30, 35, 40), border_focused: Color::Rgb(255, 180, 84),
        accent: Color::Rgb(255, 180, 84), user_msg: Color::Rgb(89, 193, 255),
        assistant_msg: Color::Rgb(168, 210, 120), system_msg: Color::Rgb(255, 180, 84),
        tool_msg: Color::Rgb(255, 142, 80), thinking: Color::Rgb(210, 168, 255),
        error: Color::Rgb(255, 102, 102), warning: Color::Rgb(255, 180, 84),
        success: Color::Rgb(168, 210, 120), info: Color::Rgb(89, 193, 255),
        dim: Color::Rgb(96, 102, 106), highlight: Color::Rgb(255, 180, 84),
        status_bar_bg: Color::Rgb(12, 16, 20), status_bar_fg: Color::Rgb(96, 102, 106),
        input_bg: Color::Rgb(12, 16, 20), input_fg: Color::Rgb(191, 189, 171),
        sidebar_bg: Color::Rgb(12, 16, 20), dialog_bg: Color::Rgb(22, 28, 33),
        dialog_border: Color::Rgb(255, 180, 84), scrollbar: Color::Rgb(30, 35, 40),
        diff_add: Color::Rgb(168, 210, 120), diff_remove: Color::Rgb(255, 102, 102),
        diff_hunk: Color::Rgb(255, 180, 84),
    }
}

fn palette_ayu_mirage() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(23, 27, 34), fg: Color::Rgb(204, 195, 163),
        border: Color::Rgb(44, 50, 58), border_focused: Color::Rgb(255, 204, 102),
        accent: Color::Rgb(255, 204, 102), user_msg: Color::Rgb(115, 218, 255),
        assistant_msg: Color::Rgb(186, 230, 126), system_msg: Color::Rgb(255, 204, 102),
        tool_msg: Color::Rgb(255, 160, 90), thinking: Color::Rgb(223, 180, 255),
        error: Color::Rgb(255, 116, 116), warning: Color::Rgb(255, 204, 102),
        success: Color::Rgb(186, 230, 126), info: Color::Rgb(115, 218, 255),
        dim: Color::Rgb(92, 99, 108), highlight: Color::Rgb(255, 204, 102),
        status_bar_bg: Color::Rgb(18, 22, 28), status_bar_fg: Color::Rgb(92, 99, 108),
        input_bg: Color::Rgb(18, 22, 28), input_fg: Color::Rgb(204, 195, 163),
        sidebar_bg: Color::Rgb(18, 22, 28), dialog_bg: Color::Rgb(30, 34, 42),
        dialog_border: Color::Rgb(255, 204, 102), scrollbar: Color::Rgb(44, 50, 58),
        diff_add: Color::Rgb(186, 230, 126), diff_remove: Color::Rgb(255, 116, 116),
        diff_hunk: Color::Rgb(255, 204, 102),
    }
}

fn palette_ayu_light() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(250, 250, 250), fg: Color::Rgb(92, 97, 102),
        border: Color::Rgb(210, 215, 218), border_focused: Color::Rgb(255, 138, 48),
        accent: Color::Rgb(255, 138, 48), user_msg: Color::Rgb(3, 155, 229),
        assistant_msg: Color::Rgb(104, 179, 56), system_msg: Color::Rgb(255, 138, 48),
        tool_msg: Color::Rgb(240, 113, 42), thinking: Color::Rgb(160, 108, 213),
        error: Color::Rgb(252, 57, 57), warning: Color::Rgb(255, 138, 48),
        success: Color::Rgb(104, 179, 56), info: Color::Rgb(3, 155, 229),
        dim: Color::Rgb(140, 144, 148), highlight: Color::Rgb(240, 113, 42),
        status_bar_bg: Color::Rgb(238, 238, 238), status_bar_fg: Color::Rgb(140, 144, 148),
        input_bg: Color::Rgb(238, 238, 238), input_fg: Color::Rgb(92, 97, 102),
        sidebar_bg: Color::Rgb(238, 238, 238), dialog_bg: Color::Rgb(245, 245, 245),
        dialog_border: Color::Rgb(255, 138, 48), scrollbar: Color::Rgb(210, 215, 218),
        diff_add: Color::Rgb(104, 179, 56), diff_remove: Color::Rgb(252, 57, 57),
        diff_hunk: Color::Rgb(255, 138, 48),
    }
}

fn palette_github_dark() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(13, 17, 23), fg: Color::Rgb(201, 209, 217),
        border: Color::Rgb(33, 38, 45), border_focused: Color::Rgb(88, 166, 255),
        accent: Color::Rgb(88, 166, 255), user_msg: Color::Rgb(88, 166, 255),
        assistant_msg: Color::Rgb(63, 185, 80), system_msg: Color::Rgb(210, 153, 34),
        tool_msg: Color::Rgb(242, 135, 40), thinking: Color::Rgb(188, 140, 255),
        error: Color::Rgb(248, 81, 73), warning: Color::Rgb(210, 153, 34),
        success: Color::Rgb(63, 185, 80), info: Color::Rgb(88, 166, 255),
        dim: Color::Rgb(125, 133, 144), highlight: Color::Rgb(242, 135, 40),
        status_bar_bg: Color::Rgb(1, 4, 9), status_bar_fg: Color::Rgb(125, 133, 144),
        input_bg: Color::Rgb(1, 4, 9), input_fg: Color::Rgb(201, 209, 217),
        sidebar_bg: Color::Rgb(1, 4, 9), dialog_bg: Color::Rgb(22, 27, 34),
        dialog_border: Color::Rgb(88, 166, 255), scrollbar: Color::Rgb(33, 38, 45),
        diff_add: Color::Rgb(63, 185, 80), diff_remove: Color::Rgb(248, 81, 73),
        diff_hunk: Color::Rgb(88, 166, 255),
    }
}

fn palette_github_light() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(255, 255, 255), fg: Color::Rgb(36, 41, 47),
        border: Color::Rgb(208, 215, 222), border_focused: Color::Rgb(9, 105, 218),
        accent: Color::Rgb(9, 105, 218), user_msg: Color::Rgb(9, 105, 218),
        assistant_msg: Color::Rgb(26, 127, 55), system_msg: Color::Rgb(154, 103, 0),
        tool_msg: Color::Rgb(188, 76, 0), thinking: Color::Rgb(130, 80, 223),
        error: Color::Rgb(207, 34, 46), warning: Color::Rgb(154, 103, 0),
        success: Color::Rgb(26, 127, 55), info: Color::Rgb(9, 105, 218),
        dim: Color::Rgb(101, 109, 118), highlight: Color::Rgb(188, 76, 0),
        status_bar_bg: Color::Rgb(246, 248, 250), status_bar_fg: Color::Rgb(101, 109, 118),
        input_bg: Color::Rgb(246, 248, 250), input_fg: Color::Rgb(36, 41, 47),
        sidebar_bg: Color::Rgb(246, 248, 250), dialog_bg: Color::Rgb(250, 250, 252),
        dialog_border: Color::Rgb(9, 105, 218), scrollbar: Color::Rgb(208, 215, 222),
        diff_add: Color::Rgb(26, 127, 55), diff_remove: Color::Rgb(207, 34, 46),
        diff_hunk: Color::Rgb(9, 105, 218),
    }
}

fn palette_vscode_dark() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(30, 30, 30), fg: Color::Rgb(212, 212, 212),
        border: Color::Rgb(51, 51, 51), border_focused: Color::Rgb(86, 156, 214),
        accent: Color::Rgb(86, 156, 214), user_msg: Color::Rgb(86, 156, 214),
        assistant_msg: Color::Rgb(96, 139, 78), system_msg: Color::Rgb(220, 220, 170),
        tool_msg: Color::Rgb(206, 145, 120), thinking: Color::Rgb(197, 134, 192),
        error: Color::Rgb(244, 71, 71), warning: Color::Rgb(220, 220, 170),
        success: Color::Rgb(96, 139, 78), info: Color::Rgb(86, 156, 214),
        dim: Color::Rgb(90, 90, 90), highlight: Color::Rgb(206, 145, 120),
        status_bar_bg: Color::Rgb(0, 122, 204), status_bar_fg: Color::Rgb(255, 255, 255),
        input_bg: Color::Rgb(37, 37, 38), input_fg: Color::Rgb(212, 212, 212),
        sidebar_bg: Color::Rgb(37, 37, 38), dialog_bg: Color::Rgb(45, 45, 45),
        dialog_border: Color::Rgb(86, 156, 214), scrollbar: Color::Rgb(66, 66, 66),
        diff_add: Color::Rgb(96, 139, 78), diff_remove: Color::Rgb(244, 71, 71),
        diff_hunk: Color::Rgb(86, 156, 214),
    }
}

fn palette_vscode_light() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(255, 255, 255), fg: Color::Rgb(30, 30, 30),
        border: Color::Rgb(230, 230, 230), border_focused: Color::Rgb(0, 92, 230),
        accent: Color::Rgb(0, 92, 230), user_msg: Color::Rgb(0, 92, 230),
        assistant_msg: Color::Rgb(0, 128, 0), system_msg: Color::Rgb(180, 130, 0),
        tool_msg: Color::Rgb(200, 100, 40), thinking: Color::Rgb(128, 0, 200),
        error: Color::Rgb(220, 40, 40), warning: Color::Rgb(180, 130, 0),
        success: Color::Rgb(0, 128, 0), info: Color::Rgb(0, 92, 230),
        dim: Color::Rgb(160, 160, 160), highlight: Color::Rgb(200, 100, 40),
        status_bar_bg: Color::Rgb(0, 120, 212), status_bar_fg: Color::Rgb(255, 255, 255),
        input_bg: Color::Rgb(250, 250, 250), input_fg: Color::Rgb(30, 30, 30),
        sidebar_bg: Color::Rgb(250, 250, 250), dialog_bg: Color::Rgb(248, 248, 248),
        dialog_border: Color::Rgb(0, 92, 230), scrollbar: Color::Rgb(200, 200, 200),
        diff_add: Color::Rgb(0, 128, 0), diff_remove: Color::Rgb(220, 40, 40),
        diff_hunk: Color::Rgb(0, 92, 230),
    }
}

fn palette_zenburn() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(63, 63, 63), fg: Color::Rgb(220, 220, 204),
        border: Color::Rgb(80, 80, 80), border_focused: Color::Rgb(140, 180, 140),
        accent: Color::Rgb(140, 180, 140), user_msg: Color::Rgb(140, 208, 211),
        assistant_msg: Color::Rgb(140, 180, 140), system_msg: Color::Rgb(224, 206, 132),
        tool_msg: Color::Rgb(220, 163, 163), thinking: Color::Rgb(204, 147, 204),
        error: Color::Rgb(220, 120, 120), warning: Color::Rgb(224, 206, 132),
        success: Color::Rgb(140, 180, 140), info: Color::Rgb(140, 208, 211),
        dim: Color::Rgb(101, 101, 101), highlight: Color::Rgb(220, 163, 163),
        status_bar_bg: Color::Rgb(54, 54, 54), status_bar_fg: Color::Rgb(101, 101, 101),
        input_bg: Color::Rgb(54, 54, 54), input_fg: Color::Rgb(220, 220, 204),
        sidebar_bg: Color::Rgb(54, 54, 54), dialog_bg: Color::Rgb(70, 70, 70),
        dialog_border: Color::Rgb(140, 180, 140), scrollbar: Color::Rgb(80, 80, 80),
        diff_add: Color::Rgb(140, 180, 140), diff_remove: Color::Rgb(220, 120, 120),
        diff_hunk: Color::Rgb(140, 180, 140),
    }
}

fn palette_oceanic_next() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(27, 43, 52), fg: Color::Rgb(205, 211, 215),
        border: Color::Rgb(40, 62, 74), border_focused: Color::Rgb(102, 153, 204),
        accent: Color::Rgb(102, 153, 204), user_msg: Color::Rgb(102, 153, 204),
        assistant_msg: Color::Rgb(153, 199, 148), system_msg: Color::Rgb(250, 200, 99),
        tool_msg: Color::Rgb(236, 95, 103), thinking: Color::Rgb(197, 148, 197),
        error: Color::Rgb(236, 95, 103), warning: Color::Rgb(250, 200, 99),
        success: Color::Rgb(153, 199, 148), info: Color::Rgb(102, 153, 204),
        dim: Color::Rgb(84, 114, 131), highlight: Color::Rgb(247, 140, 108),
        status_bar_bg: Color::Rgb(20, 34, 42), status_bar_fg: Color::Rgb(84, 114, 131),
        input_bg: Color::Rgb(20, 34, 42), input_fg: Color::Rgb(205, 211, 215),
        sidebar_bg: Color::Rgb(20, 34, 42), dialog_bg: Color::Rgb(34, 52, 63),
        dialog_border: Color::Rgb(102, 153, 204), scrollbar: Color::Rgb(40, 62, 74),
        diff_add: Color::Rgb(153, 199, 148), diff_remove: Color::Rgb(236, 95, 103),
        diff_hunk: Color::Rgb(102, 153, 204),
    }
}

fn palette_material_palenight() -> ThemePalette {
    ThemePalette {
        bg: Color::Rgb(41, 45, 62), fg: Color::Rgb(166, 172, 205),
        border: Color::Rgb(55, 60, 78), border_focused: Color::Rgb(199, 146, 234),
        accent: Color::Rgb(199, 146, 234), user_msg: Color::Rgb(130, 170, 255),
        assistant_msg: Color::Rgb(195, 232, 141), system_msg: Color::Rgb(255, 203, 107),
        tool_msg: Color::Rgb(247, 140, 108), thinking: Color::Rgb(199, 146, 234),
        error: Color::Rgb(255, 83, 112), warning: Color::Rgb(255, 203, 107),
        success: Color::Rgb(195, 232, 141), info: Color::Rgb(130, 170, 255),
        dim: Color::Rgb(103, 110, 149), highlight: Color::Rgb(247, 140, 108),
        status_bar_bg: Color::Rgb(33, 37, 52), status_bar_fg: Color::Rgb(103, 110, 149),
        input_bg: Color::Rgb(33, 37, 52), input_fg: Color::Rgb(166, 172, 205),
        sidebar_bg: Color::Rgb(33, 37, 52), dialog_bg: Color::Rgb(50, 54, 72),
        dialog_border: Color::Rgb(199, 146, 234), scrollbar: Color::Rgb(55, 60, 78),
        diff_add: Color::Rgb(195, 232, 141), diff_remove: Color::Rgb(255, 83, 112),
        diff_hunk: Color::Rgb(199, 146, 234),
    }
}
