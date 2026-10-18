//! The Gruvbox palette as named roles (from docs/design-tokens.json), the bundled font, and the
//! few widget styles more than one file uses. Colours anywhere else come from `Roles`, never hex.

use iced::widget::text::Wrapping;
use iced::widget::{button, container, text, text_input};
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
pub const GLYPH_PEN: &str = "\u{ea73}";
pub const GLYPH_LOCK: &str = "\u{ea75}";
pub const GLYPH_FILE: &str = "\u{ea7b}";
pub const GLYPH_PROJECT: &str = "\u{ea62}";
pub const GLYPH_CLAUDE: &str = "\u{ea6b}";
pub const GLYPH_UP: &str = "\u{eaa1}";
pub const GLYPH_DOWN: &str = "\u{ea9a}";

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
    pub accent: Color,
    pub accent_text: Color,
    pub accent_alt: Color,
    pub accent_cool: Color,
    pub accent_rose: Color,
    pub accent_warm: Color,
    pub accent_aqua: Color,
    /// A primary button's fill, and the text on it.
    pub btn_bg: Color,
    pub btn_fg: Color,
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
    accent: color!(0xfabd2f),
    accent_text: color!(0xfabd2f),
    accent_alt: color!(0xb8bb26),
    accent_cool: color!(0x83a598),
    accent_rose: color!(0xd3869b),
    accent_warm: color!(0xfe8019),
    accent_aqua: color!(0x8ec07c),
    btn_bg: color!(0xfabd2f),
    btn_fg: color!(0x1d2021),
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
    accent: color!(0xb57614),
    accent_text: color!(0x8a5a0e),
    accent_alt: color!(0x79740e),
    accent_cool: color!(0x076678),
    accent_rose: color!(0x8f3f71),
    accent_warm: color!(0xaf3a03),
    accent_aqua: color!(0x376b4b),
    btn_bg: color!(0x79740e),
    btn_fg: color!(0xfbf1c7),
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

    /// The colour of idea colour slot `slot` (see `project::ideas::COLOUR_SLOTS`): aqua, blue,
    /// purple, yellow, orange, green, subtle grey. Never red.
    pub fn idea(&self, slot: usize) -> Color {
        let slots = [
            self.accent_aqua,
            self.accent_cool,
            self.accent_rose,
            self.accent,
            self.accent_warm,
            self.accent_alt,
            self.fg_subtle,
        ];
        slots[slot % slots.len()]
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

/// A small raised box for tooltips and the menu.
pub fn raised_box(theme: &Theme) -> container::Style {
    let roles = Roles::of(theme);
    container::Style {
        background: Some(Background::Color(roles.bg_raised)),
        text_color: Some(roles.fg),
        border: Border::default().rounded(6).color(roles.border).width(1),
        ..container::Style::default()
    }
}

/// A dialog's card over the dimmed document: the Commit dialog, the weights view, Settings.
pub fn dialog_box(theme: &Theme) -> container::Style {
    let roles = Roles::of(theme);
    container::Style {
        background: Some(Background::Color(roles.bg_surface)),
        text_color: Some(roles.fg),
        border: Border::default().rounded(6).width(1).color(roles.border),
        ..container::Style::default()
    }
}

/// A plain coloured background, for the window and the status bar.
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

/// Small muted words on one line, such as counts beside a heading.
pub fn muted<'a>(words: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(words)
        .size(12)
        .wrapping(Wrapping::None)
        .style(|theme: &Theme| text::Style {
            color: Some(Roles::of(theme).fg_muted),
        })
}

/// Text in the subtle grey, for hints and labels.
pub fn subtle(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Roles::of(theme).fg_subtle),
    }
}

/// A text field: a thin border that turns yellow while it has focus.
pub fn input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let roles = Roles::of(theme);
    let border = match status {
        text_input::Status::Focused { .. } => roles.accent_text,
        _ => roles.border,
    };
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default().rounded(4).width(1).color(border),
        icon: roles.fg_muted,
        placeholder: roles.fg_subtle,
        value: roles.fg,
        selection: roles.accent_cool.scale_alpha(0.35),
    }
}

/// A row of a menu or panel: filled only while hovered.
pub fn menu_button(theme: &Theme, status: button::Status) -> button::Style {
    let roles = Roles::of(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(roles.bg_inset)),
        text_color: roles.fg,
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}
