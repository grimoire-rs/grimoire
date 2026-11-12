// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Every colour the TUI draws, by role.
//!
//! Render code asks [`current`] for a role and never names a colour itself;
//! the lint in `clippy.toml` keeps it that way. The `terminal` theme leaves
//! body text, backgrounds and selection to the user's own palette.
//!
//! The named palettes follow their upstream projects: Catppuccin, Gruvbox,
//! Solarized, Nord and Dracula (all MIT), and Tokyo Night (Apache-2.0).

// The one file allowed to name a colour.
#![allow(clippy::disallowed_types)]

use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

use crate::config::declaration::TuiTheme;
use crate::tui::render::ColorKey;

/// Declares [`Theme`] once so the struct, [`Theme::map_styles`] and the test
/// role table cannot drift when a role is added.
macro_rules! theme_roles {
    ($($role:ident),* $(,)?) => {
        /// One [`Style`] per UI role.
        #[derive(Debug, Clone, Copy)]
        pub struct Theme {
            $(pub $role: Style),*
        }

        impl Theme {
            /// Rebuild the theme with `f` applied to every role.
            fn map_styles(self, f: impl Fn(Style) -> Style) -> Theme {
                Theme { $($role: f(self.$role)),* }
            }

            /// Every role by name, for the tests.
            #[cfg(test)]
            fn roles(&self) -> Vec<(&'static str, Style)> {
                vec![$((stringify!($role), self.$role)),*]
            }
        }
    };
}

theme_roles! {
    text,
    muted,
    muted_dim,
    hint,
    key_hint,
    accent,
    title,
    border,
    border_popup,
    border_confirm,
    selection,
    button,
    button_selected,
    button_dimmed,
    tab_inactive,
    status_message,
    deprecated,
    deprecated_bold,
    column_registry,
    column_kind,
    column_version,
    column_rating,
    column_downloads,
    detail_label,
    detail_section,
    bullet,
    code,
    pinned,
    rule,
    gauge,
    scope_project,
    scope_global,
    scope_none,
    heading1,
    heading2,
    heading3,
    status_installed,
    status_via_bundle,
    status_not_installed,
    status_outdated,
    status_modified,
    status_pending,
    status_integrity_missing,
    status_gated,
    status_not_armed,
    status_stale,
    canvas,
}

/// Which scope the title row announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeTone {
    Project,
    Global,
    None,
}

impl Theme {
    /// Colour for a status column; call sites add `BOLD` where they want it.
    pub fn status(&self, key: ColorKey) -> Style {
        match key {
            ColorKey::Installed => self.status_installed,
            ColorKey::ViaBundle => self.status_via_bundle,
            ColorKey::NotInstalled => self.status_not_installed,
            ColorKey::Outdated => self.status_outdated,
            ColorKey::Modified => self.status_modified,
            ColorKey::Pending => self.status_pending,
            ColorKey::IntegrityMissing => self.status_integrity_missing,
            ColorKey::Gated => self.status_gated,
            ColorKey::NotArmed => self.status_not_armed,
            ColorKey::Stale => self.status_stale,
        }
    }

    /// Markdown heading style; levels 3 and deeper share one style.
    pub fn heading(&self, level: u8) -> Style {
        match level {
            1 => self.heading1,
            2 => self.heading2,
            _ => self.heading3,
        }
    }

    pub fn scope(&self, kind: ScopeTone) -> Style {
        match kind {
            ScopeTone::Project => self.scope_project,
            ScopeTone::Global => self.scope_global,
            ScopeTone::None => self.scope_none,
        }
    }
}

const BOLD: Modifier = Modifier::BOLD;
const DIM: Modifier = Modifier::DIM;
const ITALIC: Modifier = Modifier::ITALIC;
const UNDERLINED: Modifier = Modifier::UNDERLINED;

const fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

const fn fg_mod(c: Color, m: Modifier) -> Style {
    Style::new().fg(c).add_modifier(m)
}

const fn modifier(m: Modifier) -> Style {
    Style::new().add_modifier(m)
}

const PLAIN: Style = Style::new();

/// The user's terminal palette: no fixed greys, no fixed backgrounds.
pub static TERMINAL: Theme = Theme {
    canvas: PLAIN,
    text: PLAIN,
    muted: modifier(DIM),
    muted_dim: modifier(DIM),
    hint: modifier(DIM.union(ITALIC)),
    key_hint: modifier(DIM),
    accent: fg_mod(Color::Cyan, BOLD),
    title: fg_mod(Color::Magenta, BOLD),
    border: fg(Color::Blue),
    border_popup: fg(Color::Cyan),
    border_confirm: fg(Color::Yellow),
    // The accent fg makes the reversed bar one colour; without it each column inverts its own.
    // Dropping DIM keeps a `not-installed` status readable on the bar.
    selection: fg_mod(Color::Cyan, Modifier::REVERSED.union(BOLD)).remove_modifier(DIM),
    // Plain, so a locked (dimmed) button still reads differently from an unchosen one.
    button: Style::new(),
    button_selected: modifier(Modifier::REVERSED.union(BOLD)),
    button_dimmed: modifier(DIM),
    tab_inactive: PLAIN,
    status_message: fg_mod(Color::Green, BOLD),
    deprecated: fg(Color::Yellow),
    deprecated_bold: fg_mod(Color::Yellow, BOLD),
    column_registry: fg_mod(Color::Magenta, BOLD),
    column_kind: fg_mod(Color::Blue, BOLD),
    column_version: fg(Color::Yellow),
    column_rating: fg(Color::Green),
    column_downloads: fg(Color::Cyan),
    detail_label: fg_mod(Color::Blue, BOLD),
    detail_section: modifier(UNDERLINED),
    bullet: fg(Color::Blue),
    code: fg(Color::Green),
    pinned: fg(Color::Green),
    rule: modifier(DIM),
    gauge: fg(Color::Cyan),
    scope_project: fg(Color::Green),
    scope_global: fg(Color::Magenta),
    scope_none: modifier(DIM),
    heading1: fg_mod(Color::Cyan, BOLD),
    heading2: modifier(BOLD),
    heading3: modifier(UNDERLINED),
    status_installed: fg(Color::Green),
    status_via_bundle: fg(Color::Cyan),
    status_not_installed: modifier(DIM),
    status_outdated: fg(Color::Yellow),
    status_modified: fg(Color::Red),
    status_pending: fg(Color::Blue),
    status_integrity_missing: fg(Color::Magenta),
    status_gated: fg(Color::Blue),
    status_not_armed: fg(Color::Yellow),
    status_stale: fg(Color::Yellow),
};

/// The 13 colours a palette theme is built from.
///
/// `surface` is the raised background of the selected row and the gauge
/// track; `on_accent` is text readable on an `accent` background.
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub fg: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub on_accent: Color,
}

/// Every role of a fixed-palette theme, derived from its [`Palette`].
pub const fn from_palette(p: &Palette) -> Theme {
    Theme {
        canvas: Style::new().bg(p.bg).fg(p.fg),
        text: fg(p.fg),
        muted: fg(p.muted),
        muted_dim: fg_mod(p.muted, DIM),
        hint: fg_mod(p.muted, ITALIC),
        key_hint: fg(p.muted),
        accent: fg_mod(p.accent, BOLD),
        title: fg_mod(p.magenta, BOLD),
        border: fg(p.border),
        border_popup: fg(p.accent),
        border_confirm: fg(p.yellow),
        // No fg, so columns keep their own colours over the bar.
        selection: Style::new().bg(p.surface).add_modifier(BOLD),
        button: fg(p.fg),
        button_selected: Style::new().fg(p.on_accent).bg(p.accent).add_modifier(BOLD),
        button_dimmed: fg_mod(p.muted, DIM),
        tab_inactive: fg(p.fg),
        status_message: fg_mod(p.green, BOLD),
        deprecated: fg(p.yellow),
        deprecated_bold: fg_mod(p.yellow, BOLD),
        column_registry: fg_mod(p.magenta, BOLD),
        column_kind: fg_mod(p.blue, BOLD),
        column_version: fg(p.yellow),
        column_rating: fg(p.green),
        column_downloads: fg(p.cyan),
        detail_label: fg_mod(p.blue, BOLD),
        detail_section: fg_mod(p.fg, UNDERLINED),
        bullet: fg(p.blue),
        code: fg(p.green),
        pinned: fg(p.green),
        rule: fg(p.muted),
        gauge: Style::new().fg(p.accent).bg(p.surface),
        scope_project: fg(p.green),
        scope_global: fg(p.magenta),
        scope_none: fg(p.muted),
        heading1: fg_mod(p.accent, BOLD),
        heading2: fg_mod(p.fg, BOLD),
        heading3: fg_mod(p.fg, UNDERLINED),
        status_installed: fg(p.green),
        status_via_bundle: fg(p.cyan),
        status_not_installed: fg(p.muted),
        status_outdated: fg(p.yellow),
        status_modified: fg(p.red),
        status_pending: fg(p.blue),
        status_integrity_missing: fg(p.magenta),
        status_gated: fg(p.blue),
        status_not_armed: fg(p.yellow),
        status_stale: fg(p.yellow),
    }
}

/// xterm-256 palette for dark backgrounds.
pub static DARK: Theme = from_palette(&Palette {
    bg: Color::Indexed(234),
    surface: Color::Indexed(237),
    fg: Color::Indexed(252),
    muted: Color::Indexed(245),
    border: Color::Indexed(67),
    accent: Color::Indexed(80),
    red: Color::Indexed(203),
    green: Color::Indexed(114),
    yellow: Color::Indexed(179),
    blue: Color::Indexed(75),
    magenta: Color::Indexed(176),
    cyan: Color::Indexed(80),
    on_accent: Color::Indexed(234),
});

/// xterm-256 palette for light backgrounds.
pub static LIGHT: Theme = from_palette(&Palette {
    bg: Color::Indexed(255),
    surface: Color::Indexed(252),
    fg: Color::Indexed(235),
    muted: Color::Indexed(243),
    border: Color::Indexed(244),
    accent: Color::Indexed(25),
    red: Color::Indexed(124),
    green: Color::Indexed(28),
    yellow: Color::Indexed(130),
    blue: Color::Indexed(25),
    magenta: Color::Indexed(90),
    cyan: Color::Indexed(30),
    on_accent: Color::Indexed(255),
});

/// 24-bit colour from `0xRRGGBB`, so the hex stays greppable against upstream.
const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Catppuccin Latte, after the upstream catppuccin palette.
pub static CATPPUCCIN_LATTE: Theme = from_palette(&Palette {
    bg: rgb(0xeff1f5),
    surface: rgb(0xccd0da),
    fg: rgb(0x4c4f69),
    muted: rgb(0x6c6f85),
    border: rgb(0x9ca0b0),
    accent: rgb(0x8839ef),
    red: rgb(0xd20f39),
    green: rgb(0x40a02b),
    yellow: rgb(0xdf8e1d),
    blue: rgb(0x1e66f5),
    magenta: rgb(0xea76cb),
    cyan: rgb(0x179299),
    on_accent: rgb(0xdce0e8),
});

/// Catppuccin Frappe, after the upstream catppuccin palette.
pub static CATPPUCCIN_FRAPPE: Theme = from_palette(&Palette {
    bg: rgb(0x303446),
    surface: rgb(0x414559),
    fg: rgb(0xc6d0f5),
    muted: rgb(0x838ba7),
    border: rgb(0x737994),
    accent: rgb(0xca9ee6),
    red: rgb(0xe78284),
    green: rgb(0xa6d189),
    yellow: rgb(0xe5c890),
    blue: rgb(0x8caaee),
    magenta: rgb(0xf4b8e4),
    cyan: rgb(0x81c8be),
    on_accent: rgb(0x232634),
});

/// Catppuccin Macchiato, after the upstream catppuccin palette.
pub static CATPPUCCIN_MACCHIATO: Theme = from_palette(&Palette {
    bg: rgb(0x24273a),
    surface: rgb(0x363a4f),
    fg: rgb(0xcad3f5),
    muted: rgb(0x8087a2),
    border: rgb(0x6e738d),
    accent: rgb(0xc6a0f6),
    red: rgb(0xed8796),
    green: rgb(0xa6da95),
    yellow: rgb(0xeed49f),
    blue: rgb(0x8aadf4),
    magenta: rgb(0xf5bde6),
    cyan: rgb(0x8bd5ca),
    on_accent: rgb(0x181926),
});

/// Catppuccin Mocha, after the upstream catppuccin palette.
pub static CATPPUCCIN_MOCHA: Theme = from_palette(&Palette {
    bg: rgb(0x1e1e2e),
    surface: rgb(0x313244),
    fg: rgb(0xcdd6f4),
    muted: rgb(0x7f849c),
    border: rgb(0x6c7086),
    accent: rgb(0xcba6f7),
    red: rgb(0xf38ba8),
    green: rgb(0xa6e3a1),
    yellow: rgb(0xf9e2af),
    blue: rgb(0x89b4fa),
    magenta: rgb(0xf5c2e7),
    cyan: rgb(0x94e2d5),
    on_accent: rgb(0x11111b),
});

/// Gruvbox dark, after the upstream gruvbox palette.
pub static GRUVBOX_DARK: Theme = from_palette(&Palette {
    bg: rgb(0x282828),
    surface: rgb(0x3c3836),
    fg: rgb(0xebdbb2),
    muted: rgb(0x928374),
    border: rgb(0x665c54),
    accent: rgb(0xfe8019),
    red: rgb(0xfb4934),
    green: rgb(0xb8bb26),
    yellow: rgb(0xfabd2f),
    blue: rgb(0x83a598),
    magenta: rgb(0xd3869b),
    cyan: rgb(0x8ec07c),
    on_accent: rgb(0x282828),
});

/// Gruvbox light, after the upstream gruvbox palette.
pub static GRUVBOX_LIGHT: Theme = from_palette(&Palette {
    bg: rgb(0xfbf1c7),
    surface: rgb(0xebdbb2),
    fg: rgb(0x3c3836),
    muted: rgb(0x928374),
    border: rgb(0x928374),
    accent: rgb(0xaf3a03),
    red: rgb(0x9d0006),
    green: rgb(0x79740e),
    yellow: rgb(0xb57614),
    blue: rgb(0x076678),
    magenta: rgb(0x8f3f71),
    cyan: rgb(0x427b58),
    on_accent: rgb(0xfbf1c7),
});

/// Solarized dark, after the upstream Solarized palette.
pub static SOLARIZED_DARK: Theme = from_palette(&Palette {
    bg: rgb(0x002b36),
    surface: rgb(0x073642),
    fg: rgb(0x839496),
    muted: rgb(0x657b83),
    border: rgb(0x586e75),
    accent: rgb(0x268bd2),
    red: rgb(0xdc322f),
    green: rgb(0x859900),
    yellow: rgb(0xb58900),
    blue: rgb(0x268bd2),
    magenta: rgb(0xd33682),
    cyan: rgb(0x2aa198),
    on_accent: rgb(0x002b36),
});

/// Solarized light, after the upstream Solarized palette.
pub static SOLARIZED_LIGHT: Theme = from_palette(&Palette {
    bg: rgb(0xfdf6e3),
    surface: rgb(0xeee8d5),
    fg: rgb(0x657b83),
    muted: rgb(0x586e75),
    border: rgb(0x93a1a1),
    accent: rgb(0x268bd2),
    red: rgb(0xdc322f),
    green: rgb(0x859900),
    yellow: rgb(0xb58900),
    blue: rgb(0x268bd2),
    magenta: rgb(0xd33682),
    cyan: rgb(0x2aa198),
    on_accent: rgb(0xfdf6e3),
});

/// Tokyo Night (night variant), after the upstream tokyonight.nvim palette.
pub static TOKYO_NIGHT: Theme = from_palette(&Palette {
    bg: rgb(0x1a1b26),
    surface: rgb(0x292e42),
    fg: rgb(0xc0caf5),
    muted: rgb(0x737aa2),
    border: rgb(0x3d59a1),
    accent: rgb(0x7aa2f7),
    red: rgb(0xf7768e),
    green: rgb(0x9ece6a),
    yellow: rgb(0xe0af68),
    blue: rgb(0x7aa2f7),
    magenta: rgb(0xbb9af7),
    cyan: rgb(0x7dcfff),
    on_accent: rgb(0x1a1b26),
});

/// Nord, after the upstream Nord palette.
pub static NORD: Theme = from_palette(&Palette {
    bg: rgb(0x2e3440),
    surface: rgb(0x3b4252),
    fg: rgb(0xd8dee9),
    muted: rgb(0x616e88),
    border: rgb(0x616e88),
    accent: rgb(0x88c0d0),
    red: rgb(0xbf616a),
    green: rgb(0xa3be8c),
    yellow: rgb(0xebcb8b),
    blue: rgb(0x81a1c1),
    magenta: rgb(0xb48ead),
    cyan: rgb(0x8fbcbb),
    on_accent: rgb(0x2e3440),
});

/// Dracula, after the upstream Dracula palette.
pub static DRACULA: Theme = from_palette(&Palette {
    bg: rgb(0x282a36),
    surface: rgb(0x44475a),
    fg: rgb(0xf8f8f2),
    muted: rgb(0x6272a4),
    border: rgb(0x6272a4),
    accent: rgb(0xbd93f9),
    red: rgb(0xff5555),
    green: rgb(0x50fa7b),
    yellow: rgb(0xf1fa8c),
    blue: rgb(0xbd93f9),
    magenta: rgb(0xff79c6),
    cyan: rgb(0x8be9fd),
    on_accent: rgb(0x282a36),
});

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// Nearest xterm-256 index: the closer of the best colour-cube cell and the
/// best grey-ramp step.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |c: u8| {
        (0u8..6)
            .min_by_key(|&i| CUBE_LEVELS[usize::from(i)].abs_diff(c))
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (
        CUBE_LEVELS[usize::from(ri)],
        CUBE_LEVELS[usize::from(gi)],
        CUBE_LEVELS[usize::from(bi)],
    );
    let step = (0u8..24)
        .min_by_key(|&i| dist2((r, g, b), (8 + 10 * i, 8 + 10 * i, 8 + 10 * i)))
        .unwrap_or(0);
    let grey = 8 + 10 * step;
    if dist2((r, g, b), cube_rgb) <= dist2((r, g, b), (grey, grey, grey)) {
        cube
    } else {
        232 + step
    }
}

fn downsample_colour(c: Option<Color>) -> Option<Color> {
    match c {
        Some(Color::Rgb(r, g, b)) => Some(Color::Indexed(nearest_256(r, g, b))),
        other => other,
    }
}

impl Theme {
    /// The theme with every 24-bit colour replaced by its xterm-256 neighbour.
    fn downsample(self) -> Theme {
        self.map_styles(|s| Style {
            fg: downsample_colour(s.fg),
            bg: downsample_colour(s.bg),
            ..s
        })
    }
}

static CURRENT: OnceLock<Theme> = OnceLock::new();

/// Select the process-wide theme; without `truecolor`, 24-bit colours are
/// reduced to the 256-colour palette. The first call wins; no unit test may
/// call this, since the state is global.
pub fn init(name: TuiTheme, truecolor: bool) {
    let base = match name {
        TuiTheme::Terminal => TERMINAL,
        TuiTheme::Dark => DARK,
        TuiTheme::Light => LIGHT,
        TuiTheme::CatppuccinLatte => CATPPUCCIN_LATTE,
        TuiTheme::CatppuccinFrappe => CATPPUCCIN_FRAPPE,
        TuiTheme::CatppuccinMacchiato => CATPPUCCIN_MACCHIATO,
        TuiTheme::CatppuccinMocha => CATPPUCCIN_MOCHA,
        TuiTheme::GruvboxDark => GRUVBOX_DARK,
        TuiTheme::GruvboxLight => GRUVBOX_LIGHT,
        TuiTheme::SolarizedDark => SOLARIZED_DARK,
        TuiTheme::SolarizedLight => SOLARIZED_LIGHT,
        TuiTheme::TokyoNight => TOKYO_NIGHT,
        TuiTheme::Nord => NORD,
        TuiTheme::Dracula => DRACULA,
    };
    let _ = CURRENT.set(if truecolor { base } else { base.downsample() });
}

/// The selected theme, `terminal` until [`init`] runs.
pub fn current() -> &'static Theme {
    CURRENT.get().unwrap_or(&TERMINAL)
}

/// Fill the canvas colours into every cell the widgets left at the terminal
/// default. A post-pass, not a background `Block`: `Clear` in popups resets
/// cells to the default and would punch holes in a painted background.
pub fn paint_canvas(buf: &mut Buffer) {
    paint(current().canvas, buf);
}

fn paint(canvas: Style, buf: &mut Buffer) {
    if canvas.bg.is_none() && canvas.fg.is_none() {
        return;
    }
    for cell in &mut buf.content {
        if let Some(bg) = canvas.bg.filter(|_| cell.bg == Color::Reset) {
            cell.bg = bg;
        }
        if let Some(fg) = canvas.fg.filter(|_| cell.fg == Color::Reset) {
            cell.fg = fg;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KEYS: [ColorKey; 10] = [
        ColorKey::Installed,
        ColorKey::ViaBundle,
        ColorKey::NotInstalled,
        ColorKey::Outdated,
        ColorKey::Modified,
        ColorKey::Pending,
        ColorKey::IntegrityMissing,
        ColorKey::Gated,
        ColorKey::NotArmed,
        ColorKey::Stale,
    ];

    fn allowed(c: Option<Color>) -> bool {
        matches!(
            c,
            None | Some(
                Color::Reset
                    | Color::Red
                    | Color::Green
                    | Color::Yellow
                    | Color::Blue
                    | Color::Magenta
                    | Color::Cyan
                    | Color::LightRed
                    | Color::LightGreen
                    | Color::LightYellow
                    | Color::LightBlue
                    | Color::LightMagenta
                    | Color::LightCyan
            )
        )
    }

    #[test]
    fn terminal_theme_uses_only_palette_following_colours() {
        let mut styles = TERMINAL.roles();
        for key in ALL_KEYS {
            styles.push(("status()", TERMINAL.status(key)));
        }
        for level in 1..=3 {
            styles.push(("heading()", TERMINAL.heading(level)));
        }
        for tone in [ScopeTone::Project, ScopeTone::Global, ScopeTone::None] {
            styles.push(("scope()", TERMINAL.scope(tone)));
        }
        for (name, s) in styles {
            assert!(
                allowed(s.fg),
                "terminal `{name}` fg {:?} is not palette-following",
                s.fg
            );
            assert!(
                allowed(s.bg),
                "terminal `{name}` bg {:?} is not palette-following",
                s.bg
            );
        }
    }

    /// Clippy's `disallowed-types` flags imports but misses `Color::X` through
    /// a fully-qualified path or `ratatui::prelude::*`, so scan the source too.
    #[test]
    fn no_raw_colours_outside_theme() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, hits);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("tui/theme.rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        if has_raw_colour(line) {
                            hits.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
        assert!(hits.is_empty(), "raw colours outside theme.rs:\n{}", hits.join("\n"));
    }

    /// `Color::` not preceded by an identifier char (so `ColorKey::` and
    /// `TermColor::` do not match), `Stylize`, or a ratatui prelude glob.
    fn has_raw_colour(line: &str) -> bool {
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let bare_color = line
            .match_indices("Color::")
            .any(|(i, _)| !line[..i].chars().next_back().is_some_and(ident));
        bare_color || line.contains("Stylize") || line.contains("ratatui::prelude")
    }

    #[test]
    fn raw_colour_matcher_distinguishes_colorkey() {
        assert!(has_raw_colour("fg(Color::Cyan)"));
        assert!(has_raw_colour("ratatui::style::Color::Red"));
        assert!(has_raw_colour("use ratatui::style::Stylize;"));
        assert!(!has_raw_colour("ColorKey::Installed"));
        assert!(!has_raw_colour("let k = MyColor::X;"));
    }

    #[test]
    fn current_is_terminal_until_init() {
        assert!(std::ptr::eq(current(), &TERMINAL));
    }

    /// Every non-terminal built-in theme; a new palette theme adds one line.
    const BUILT_IN: &[(&str, &Theme)] = &[
        ("dark", &DARK),
        ("light", &LIGHT),
        ("catppuccin-latte", &CATPPUCCIN_LATTE),
        ("catppuccin-frappe", &CATPPUCCIN_FRAPPE),
        ("catppuccin-macchiato", &CATPPUCCIN_MACCHIATO),
        ("catppuccin-mocha", &CATPPUCCIN_MOCHA),
        ("gruvbox-dark", &GRUVBOX_DARK),
        ("gruvbox-light", &GRUVBOX_LIGHT),
        ("solarized-dark", &SOLARIZED_DARK),
        ("solarized-light", &SOLARIZED_LIGHT),
        ("tokyo-night", &TOKYO_NIGHT),
        ("nord", &NORD),
        ("dracula", &DRACULA),
    ];

    #[test]
    fn dark_status_follows_its_palette() {
        let green = Some(Color::Indexed(114));
        assert_eq!(DARK.status(ColorKey::Installed).fg, green);
        assert_eq!(DARK.status(ColorKey::NotInstalled).fg, Some(Color::Indexed(245)));
        assert_eq!(DARK.status(ColorKey::Modified).fg, Some(Color::Indexed(203)));
        assert_eq!(DARK.status(ColorKey::Gated).fg, Some(Color::Indexed(75)));
        assert_eq!(DARK.status(ColorKey::Stale).fg, Some(Color::Indexed(179)));
    }

    fn rgb_of(c: Color) -> Option<(u8, u8, u8)> {
        match c {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Indexed(i @ 16..=231) => {
                let n = i - 16;
                let l = |k: u8| CUBE_LEVELS[usize::from(k)];
                Some((l(n / 36), l(n / 6 % 6), l(n % 6)))
            }
            Color::Indexed(i @ 232..=255) => {
                let v = 8 + 10 * (i - 232);
                Some((v, v, v))
            }
            _ => None,
        }
    }

    fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Official theme hues are kept even where they are soft, so non-text roles
    /// only need 2.2 and text 4.0 (Solarized Light's own body text is 4.13);
    /// do not "fix" a palette to satisfy this test.
    #[test]
    fn palette_themes_keep_every_text_role_readable() {
        for (name, theme) in BUILT_IN {
            let canvas = theme.canvas.bg.and_then(rgb_of);
            let canvas = canvas.unwrap_or_else(|| panic!("{name}: canvas bg is not an RGB-resolvable colour"));
            for (role, style) in theme.roles() {
                // The gauge is a fill/track pair, not text.
                if role == "gauge" {
                    continue;
                }
                let Some(fg) = style.fg else { continue };
                let fg = rgb_of(fg).unwrap_or_else(|| panic!("{name} `{role}`: fg {fg:?} has no RGB"));
                let bg = style.bg.and_then(rgb_of).unwrap_or(canvas);
                let ratio = contrast(fg, bg);
                let floor = if role == "text" { 4.0 } else { 2.2 };
                assert!(ratio >= floor, "{name} `{role}`: contrast {ratio:.2} < {floor}");
            }
        }
    }

    #[test]
    fn downsampled_built_ins_keep_text_readable() {
        for (name, theme) in BUILT_IN {
            let t = (**theme).downsample();
            for (role, s) in t.roles() {
                for c in [s.fg, s.bg].into_iter().flatten() {
                    assert!(!matches!(c, Color::Rgb(..)), "{name} `{role}` still has {c:?}");
                }
            }
            let fg = t.text.fg.and_then(rgb_of).expect("text fg");
            let bg = t.canvas.bg.and_then(rgb_of).expect("canvas bg");
            let ratio = contrast(fg, bg);
            assert!(ratio >= 3.5, "{name} downsampled text contrast {ratio:.2} < 3.5");
        }
    }

    #[test]
    fn nearest_256_known_points() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(128, 128, 128), 244);
    }

    #[test]
    fn nearest_256_beats_the_obvious_alternatives() {
        let rgb = (30, 30, 46);
        let idx = nearest_256(rgb.0, rgb.1, rgb.2);
        let d = |i: u8| dist2(rgb, rgb_of(Color::Indexed(i)).unwrap_or((0, 0, 0)));
        assert!(d(idx) <= d(16) && d(idx) <= d(235), "picked {idx}");
    }

    #[test]
    fn downsample_leaves_no_rgb_and_keeps_other_colours() {
        let t = from_palette(&Palette {
            bg: Color::Rgb(30, 30, 46),
            surface: Color::Rgb(49, 50, 68),
            fg: Color::Rgb(205, 214, 244),
            muted: Color::Red,
            border: Color::Rgb(108, 112, 134),
            accent: Color::Rgb(203, 166, 247),
            red: Color::Rgb(243, 139, 168),
            green: Color::Rgb(166, 227, 161),
            yellow: Color::Rgb(249, 226, 175),
            blue: Color::Rgb(137, 180, 250),
            magenta: Color::Rgb(245, 194, 231),
            cyan: Color::Rgb(148, 226, 213),
            on_accent: Color::Rgb(30, 30, 46),
        })
        .downsample();
        for (role, s) in t.roles() {
            for c in [s.fg, s.bg].into_iter().flatten() {
                assert!(!matches!(c, Color::Rgb(..)), "`{role}` still has {c:?}");
            }
        }
        assert_eq!(t.muted.fg, Some(Color::Red));
    }

    #[test]
    fn paint_fills_only_default_cells() {
        use ratatui::layout::Rect;
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.content[1].bg = Color::Blue;
        paint(Style::new().bg(Color::Indexed(234)).fg(Color::Indexed(252)), &mut buf);
        assert_eq!(buf.content[0].bg, Color::Indexed(234));
        assert_eq!(buf.content[0].fg, Color::Indexed(252));
        assert_eq!(buf.content[1].bg, Color::Blue);
        let mut plain = Buffer::empty(Rect::new(0, 0, 1, 1));
        paint(PLAIN, &mut plain);
        assert_eq!(plain.content[0].bg, Color::Reset);
    }
}
