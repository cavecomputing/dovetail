//! The Gruvbox palette as named roles (from docs/design-tokens.json), the bundled font, and the
//! few widget styles more than one file uses. Colours anywhere else come from `Roles`, never hex.

use iced::widget::{button, container};
use iced::{Background, Border, Color, Font, Theme, color};

pub const FONT_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFontMono-Regular.ttf");
pub const FONT_BOLD: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFontMono-Bold.ttf");
pub const FONT_ITALIC: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFontMono-Italic.ttf");

/// The family name inside the bundled font files (what `fc-scan` reports).
pub const MONO: Font = Font::with_name("JetBrainsMonoNL Nerd Font Mono");

/// Nerd Font glyphs used as icons (codicons).
pub const GLYPH_MINIMIZE: &str = "\u{eaba}";
pub const GLYPH_MAXIMIZE: &str = "\u{eab9}";
pub const GLYPH_CLOSE: &str = "\u{eab8}";
pub const GLYPH_MENU: &str = "\u{eb94}";
pub const GLYPH_CLAUDE: &str = "\u{ea6b}";
pub const GLYPH_PEN: &str = "\u{ea73}";
pub const GLYPH_UP: &str = "\u{eaa1}";
pub const GLYPH_DOWN: &str = "\u{ea9a}";
pub const GLYPH_CASE: &str = "\u{eab1}";
pub const GLYPH_WHOLE_WORD: &str = "\u{eb7e}";

/// The editor's mono font is about 0.6 of its size wide per character.
pub const CHAR_WIDTH_RATIO: f32 = 0.6;

pub struct Roles {
    pub bg_page: Color,
    pub bg_surface: Color,
    pub bg_raised: Color,
    pub bg_inset: Color,
    pub border: Color,
    pub fg: Color,
    pub fg_muted: Color,
    pub fg_subtle: Color,
    pub accent_text: Color,
    pub accent_cool: Color,
    pub accent_warm: Color,
    pub accent_aqua: Color,
    /// Behind text a suggested edit takes out (Gruvbox red, the app's only red) and puts in
    /// (`accent-alt`, green), drawn see-through so the text on them stays `fg`.
    pub removed: Color,
    pub added: Color,
}

pub const DARK: Roles = Roles {
    bg_page: color!(0x1d2021),
    bg_surface: color!(0x282828),
    bg_raised: color!(0x32302f),
    bg_inset: color!(0x3c3836),
    border: color!(0x3c3836),
    fg: color!(0xebdbb2),
    fg_muted: color!(0xa89984),
    fg_subtle: color!(0x7c6f64),
    accent_text: color!(0xfabd2f),
    accent_cool: color!(0x83a598),
    accent_warm: color!(0xfe8019),
    accent_aqua: color!(0x8ec07c),
    removed: color!(0xfb4934),
    added: color!(0xb8bb26),
};

pub const LIGHT: Roles = Roles {
    bg_page: color!(0xf9f5d7),
    bg_surface: color!(0xfbf1c7),
    bg_raised: color!(0xf2e5bc),
    bg_inset: color!(0xebdbb2),
    border: color!(0xe0d3ac),
    fg: color!(0x3c3836),
    fg_muted: color!(0x665c54),
    fg_subtle: color!(0x7c6f64),
    accent_text: color!(0x8a5a0e),
    accent_cool: color!(0x076678),
    accent_warm: color!(0xaf3a03),
    accent_aqua: color!(0x376b4b),
    removed: color!(0x9d0006),
    added: color!(0x79740e),
};

impl Roles {
    /// The roles behind a theme built by `gruvbox`.
    pub fn of(theme: &Theme) -> &'static Roles {
        if theme.extended_palette().is_dark {
            &DARK
        } else {
            &LIGHT
        }
    }
}

/// Builds the iced theme once from the roles; the app keeps it and hands out clones.
pub fn gruvbox(dark: bool) -> Theme {
    let roles = if dark { &DARK } else { &LIGHT };
    let name = if dark {
        "Gruvbox dark"
    } else {
        "Gruvbox light"
    };
    Theme::custom(
        name,
        iced::theme::Palette {
            background: roles.bg_surface,
            text: roles.fg,
            primary: roles.accent_text,
            success: roles.accent_aqua,
            warning: roles.accent_warm,
            // No red, ever: errors use orange too.
            danger: roles.accent_warm,
        },
    )
}

/// A plain glyph button: no fill until hovered.
pub fn icon_button(theme: &Theme, status: button::Status) -> button::Style {
    let roles = Roles::of(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(roles.border)),
        text_color: if hovered { roles.fg } else { roles.fg_muted },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

/// A small raised box for tooltips, the menu and the panels over the text.
pub fn raised_box(theme: &Theme) -> container::Style {
    let roles = Roles::of(theme);
    container::Style {
        background: Some(Background::Color(roles.bg_raised)),
        text_color: Some(roles.fg),
        border: Border::default().rounded(6).color(roles.border).width(1),
        ..container::Style::default()
    }
}

/// A plain coloured background, for the window, the status bar, the panes and the dividers.
pub fn fill(color: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        ..container::Style::default()
    }
}

/// The bundled font in bold.
pub fn bold() -> Font {
    Font {
        weight: iced::font::Weight::Bold,
        ..MONO
    }
}
