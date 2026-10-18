//! The Settings panel (Ctrl+,), over the dimmed document. Every change applies at once and is
//! written to settings.toml.

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, text, text_input,
};
use iced::{Background, Border, Color, Element, Fill, Padding, Theme};

use crate::ai::key::KeyStore;
use crate::core::paths::display_path;
use crate::core::settings::{
    DEFAULT_BASE_URL, ReadingMode, RequestFormat, Settings, Theme as ThemeSetting, WindowControls,
};
use crate::ui::app::{App, Message};
use crate::ui::assistant_actions::AssistantAction;
use crate::ui::status_bar::dot;
use crate::ui::theme::{self, GLYPH_CLOSE, Roles, dialog_box, icon_button, subtle};

/// One edit made in the Settings panel.
#[derive(Debug, Clone)]
pub enum Change {
    Theme(ThemeSetting),
    FontSize(f32),
    ReadingMode(ReadingMode),
    TabSpaces(u8),
    WordWrap(bool),
    ColourMarkup(bool),
    ShowLineCol(bool),
    AutosaveDelay(u64),
    BaseUrl(String),
    Model(String),
    RequestFormat(RequestFormat),
    WindowControls(WindowControls),
}

/// The assistant's API key field. The key is saved as it changes, off the UI thread, since the
/// keyring may be slow; it never goes through `Change`.
#[derive(Debug, Default)]
pub struct KeyField {
    pub text: String,
    /// Where keys are kept on this machine; `None` until checked, when Settings opens.
    pub store: Option<KeyStore>,
    pub saving: bool,
    pub error: Option<String>,
}

/// How the last "test connection" went.
#[derive(Debug, Clone)]
pub enum Connection {
    Testing,
    /// It answered, with this text (maybe empty).
    Passed(String),
    Failed(String),
}

/// Applies `change`, then keeps the numbers in range (`Settings::clamp` holds the limits).
pub fn apply(change: Change, settings: &mut Settings) {
    match change {
        Change::Theme(theme) => settings.theme = theme,
        Change::FontSize(size) => settings.font_size = size,
        Change::ReadingMode(mode) => settings.reading_mode = mode,
        Change::TabSpaces(spaces) => settings.tab_spaces = spaces,
        Change::WordWrap(on) => settings.word_wrap = on,
        Change::ColourMarkup(on) => settings.colour_markup = on,
        Change::ShowLineCol(on) => settings.show_line_col = on,
        Change::AutosaveDelay(ms) => settings.autosave_delay_ms = ms,
        Change::BaseUrl(url) => settings.base_url = url,
        Change::Model(model) => settings.model = model,
        Change::RequestFormat(format) => settings.request_format = format,
        Change::WindowControls(controls) => settings.window_controls = controls,
    }
    settings.clamp();
}

pub fn view(app: &App) -> Element<'_, Message> {
    let settings = &app.settings;
    // The comfort width is remembered while full width is on, so switching back keeps it.
    let comfort_chars = match settings.reading_mode {
        ReadingMode::Comfort { chars } => chars,
        ReadingMode::FullWidth => app.last_comfort_chars,
    };
    let comfort = matches!(settings.reading_mode, ReadingMode::Comfort { .. });
    let comfort_with = |chars| Change::ReadingMode(ReadingMode::Comfort { chars });

    let appearance = section(
        "appearance",
        vec![
            setting_row(
                "Theme",
                choices(vec![
                    (
                        "dark",
                        settings.theme == ThemeSetting::Dark,
                        Change::Theme(ThemeSetting::Dark),
                    ),
                    (
                        "light",
                        settings.theme == ThemeSetting::Light,
                        Change::Theme(ThemeSetting::Light),
                    ),
                    (
                        "system",
                        settings.theme == ThemeSetting::System,
                        Change::Theme(ThemeSetting::System),
                    ),
                ]),
            ),
            setting_row(
                "Font",
                row![value_text("JetBrains Mono NL Nerd Font"), hint("bundled")]
                    .align_y(iced::Center)
                    .into(),
            ),
            setting_row(
                "Font size",
                stepper(
                    format!("{}", settings.font_size),
                    "px",
                    Change::FontSize(settings.font_size - 1.0),
                    Change::FontSize(settings.font_size + 1.0),
                ),
            ),
            setting_row(
                "Reading mode",
                row![
                    choices(vec![
                        (
                            "full width",
                            !comfort,
                            Change::ReadingMode(ReadingMode::FullWidth)
                        ),
                        ("comfort", comfort, comfort_with(comfort_chars)),
                    ]),
                    separator("·"),
                    stepper(
                        comfort_chars.to_string(),
                        "chars a line",
                        comfort_with(comfort_chars.saturating_sub(5)),
                        comfort_with(comfort_chars + 5),
                    ),
                ]
                .align_y(iced::Center)
                .into(),
            ),
        ],
    );

    let editor = section(
        "editor",
        vec![
            setting_row(
                "Tab inserts",
                stepper(
                    settings.tab_spaces.to_string(),
                    "spaces, never tabs",
                    Change::TabSpaces(settings.tab_spaces.saturating_sub(1)),
                    Change::TabSpaces(settings.tab_spaces.saturating_add(1)),
                ),
            ),
            setting_row(
                "Word wrap",
                row![
                    on_off(settings.word_wrap, Change::WordWrap),
                    hint("off cuts long lines at the edge")
                ]
                .align_y(iced::Center)
                .into(),
            ),
            setting_row(
                "XML and macro colour",
                on_off(settings.colour_markup, Change::ColourMarkup),
            ),
            setting_row(
                "Line and column",
                on_off(settings.show_line_col, Change::ShowLineCol),
            ),
        ],
    );

    let delay = settings.autosave_delay_ms;
    let saving = section(
        "saving",
        vec![setting_row(
            "Autosave after",
            stepper(
                delay.to_string(),
                "ms pause in typing",
                Change::AutosaveDelay(delay.saturating_sub(250)),
                Change::AutosaveDelay(delay + 250),
            ),
        )],
    );

    let format = settings.request_format;
    let assistant = section(
        "assistant",
        vec![
            setting_row(
                "Base URL",
                field(DEFAULT_BASE_URL, &settings.base_url, Change::BaseUrl),
            ),
            setting_row(
                "Model",
                field(
                    "model name, as the server knows it",
                    &settings.model,
                    Change::Model,
                ),
            ),
            setting_row(
                "Request format",
                choices(vec![
                    (
                        "responses",
                        format == RequestFormat::Responses,
                        Change::RequestFormat(RequestFormat::Responses),
                    ),
                    (
                        "chat completions",
                        format == RequestFormat::ChatCompletions,
                        Change::RequestFormat(RequestFormat::ChatCompletions),
                    ),
                ]),
            ),
            setting_row("API key", key_input(&app.key_field.text)),
            setting_row("", key_status(&app.key_field)),
            setting_row(
                "Connection",
                value_button(
                    "test connection".to_string(),
                    None,
                    Message::Assistant(AssistantAction::TestConnection),
                ),
            ),
            connection_result(app.connection.as_ref()),
        ],
    );

    let window = section(
        "window",
        vec![setting_row(
            "Window controls",
            row![
                choices(vec![
                    (
                        "auto",
                        settings.window_controls == WindowControls::Auto,
                        Change::WindowControls(WindowControls::Auto)
                    ),
                    (
                        "show",
                        settings.window_controls == WindowControls::Show,
                        Change::WindowControls(WindowControls::Show)
                    ),
                    (
                        "hide",
                        settings.window_controls == WindowControls::Hide,
                        Change::WindowControls(WindowControls::Hide)
                    ),
                ]),
                hint("auto hides them on tiling WMs"),
            ]
            .align_y(iced::Center)
            .into(),
        )],
    );

    let mut sections = column![appearance, editor, saving, assistant].spacing(28);
    // macOS keeps its own window buttons, so there is nothing to choose.
    if !cfg!(target_os = "macos") {
        sections = sections.push(window);
    }
    // Taller than the window, it scrolls, with the bar in the card's right margin.
    let page = scrollable(container(sections).padding([0, 32]));

    // Where the settings are saved, or why they can't be.
    let roles = Roles::of(&app.theme);
    let (dot_color, color, file) = match &app.settings_error {
        Some(reason) => (roles.accent_warm, roles.accent_warm, reason.clone()),
        None => (
            roles.accent_aqua,
            roles.fg_muted,
            display_path(&app.paths.settings_file()),
        ),
    };
    let close = button(text(GLYPH_CLOSE).size(13).center())
        .width(28)
        .height(24)
        .padding(0)
        .style(icon_button)
        .on_press(Message::CloseMenu);
    let header = row![
        text("Settings").size(13),
        dot(dot_color),
        text(file).size(12).color(color),
        space::horizontal(),
        // Hidden while there's a problem, which needs the room and isn't instant.
        hint(if app.settings_error.is_some() {
            ""
        } else {
            "changes apply instantly"
        }),
        close,
    ]
    .spacing(8)
    .align_y(iced::Center)
    .padding(Padding {
        left: 32.0,
        right: 12.0,
        ..Padding::ZERO
    });
    let card = container(column![header, page].spacing(20))
        .width(Fill)
        .max_width(704)
        .padding(Padding {
            top: 12.0,
            bottom: 28.0,
            ..Padding::ZERO
        })
        .style(dialog_box);
    modal(card.into())
}

/// `card` centred over a dimmed window; clicking outside it goes back.
fn modal(card: Element<'_, Message>) -> Element<'_, Message> {
    iced::widget::stack![
        mouse_area(
            container(space().width(Fill).height(Fill)).style(|theme: &Theme| container::Style {
                background: Some(Background::Color(Roles::of(theme).bg_page.scale_alpha(0.7))),
                ..container::Style::default()
            })
        )
        .on_press(Message::CloseMenu),
        container(iced::widget::opaque(card))
            .center(Fill)
            .padding(24),
    ]
    .into()
}

fn section<'a>(label: &'a str, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let rule = container(space().width(Fill).height(1))
        .style(|theme: &Theme| theme::fill(Roles::of(theme).border));
    let heading = row![text(label).size(12).style(subtle), rule]
        .spacing(10)
        .align_y(iced::Center);
    column![heading, column(rows)].spacing(6).into()
}

fn setting_row<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![
        text(label)
            .size(13)
            .width(200)
            .style(|theme: &Theme| text::Style {
                color: Some(Roles::of(theme).fg_muted),
            }),
        control,
    ]
    .spacing(16)
    .height(30)
    .align_y(iced::Center)
    .into()
}

/// Options side by side, separated by `/`; the chosen one is yellow.
fn choices<'a>(options: Vec<(&'a str, bool, Change)>) -> Element<'a, Message> {
    let mut line = row![].align_y(iced::Center);
    for (index, (label, chosen, change)) in options.into_iter().enumerate() {
        if index > 0 {
            line = line.push(separator("/"));
        }
        let message = Message::Setting(change);
        line = line.push(value_button(label.to_string(), Some(chosen), message));
    }
    line.into()
}

fn on_off<'a>(on: bool, change: fn(bool) -> Change) -> Element<'a, Message> {
    choices(vec![("on", on, change(true)), ("off", !on, change(false))])
}

/// A number with − and + on either side, and its unit after.
fn stepper<'a>(value: String, unit: &'a str, less: Change, more: Change) -> Element<'a, Message> {
    row![
        value_button("−".to_string(), None, Message::Setting(less)),
        value_text(value),
        value_button("+".to_string(), None, Message::Setting(more)),
        hint(unit),
    ]
    .align_y(iced::Center)
    .into()
}

/// A text box for a free-form setting; empty shows `placeholder`.
fn field<'a>(placeholder: &str, value: &str, change: fn(String) -> Change) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(move |typed| Message::Setting(change(typed)))
        .size(13)
        .padding([3, 8])
        .width(320)
        .style(field_style)
        .into()
}

/// The API key, shown as dots. Each change is saved (see `KeyField`); empty deletes it.
fn key_input<'a>(key: &str) -> Element<'a, Message> {
    text_input("paste a key, if the server needs one", key)
        .secure(true)
        .on_input(|typed| Message::Assistant(AssistantAction::KeyTyped(typed)))
        .size(13)
        .padding([3, 8])
        .width(320)
        .style(field_style)
        .into()
}

/// Where the key is kept, in words: the keyring, or settings.toml when there is none.
fn key_status<'a>(field: &KeyField) -> Element<'a, Message> {
    if let Some(error) = &field.error {
        return warning(error.clone());
    }
    let words = match (field.store, field.saving, field.text.trim().is_empty()) {
        (None, _, _) => "checking for a keyring…",
        (_, true, _) => "saving…",
        (Some(KeyStore::Keyring), _, false) => "stored in the system keyring",
        (Some(KeyStore::Keyring), _, true) => "none saved · a key goes in the system keyring",
        (Some(KeyStore::SettingsFile), _, false) => "no keyring found, stored in settings.toml",
        (Some(KeyStore::SettingsFile), _, true) => {
            "none saved · no keyring, a key goes in settings.toml"
        }
    };
    hint(words)
}

/// The test's outcome under the button: what the model replied, or why it failed.
fn connection_result<'a>(connection: Option<&Connection>) -> Element<'a, Message> {
    let line = match connection {
        None => return space().into(),
        Some(Connection::Testing) => hint("testing…"),
        Some(Connection::Passed(reply)) if reply.trim().is_empty() => {
            ok_line("ok · it answered".to_string())
        }
        Some(Connection::Passed(reply)) => {
            let reply: String = reply.trim().chars().take(60).collect();
            ok_line(format!("ok · it replied \u{201c}{reply}\u{201d}"))
        }
        Some(Connection::Failed(reason)) => warning(reason.clone()),
    };
    // Under the controls, wrapping, so a long reason stays readable.
    container(line)
        .padding(iced::Padding {
            left: 216.0,
            ..iced::Padding::ZERO
        })
        .into()
}

fn ok_line<'a>(words: String) -> Element<'a, Message> {
    container(text(words).size(12).style(|theme: &Theme| text::Style {
        color: Some(Roles::of(theme).accent_aqua),
    }))
    .padding([0, 8])
    .into()
}

/// A problem in words, in the warm accent (never red).
fn warning<'a>(words: String) -> Element<'a, Message> {
    container(text(words).size(12).style(|theme: &Theme| text::Style {
        color: Some(Roles::of(theme).accent_warm),
    }))
    .padding([0, 8])
    .into()
}

/// A field's frame shows only on hover, and turns yellow while it has focus.
fn field_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let roles = Roles::of(theme);
    let border = match status {
        text_input::Status::Focused { .. } => roles.accent_text,
        text_input::Status::Hovered => roles.border,
        _ => Color::TRANSPARENT,
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

/// A clickable value. `chosen` colours it as an option: yellow if picked, dim if not.
fn value_button<'a>(label: String, chosen: Option<bool>, message: Message) -> Element<'a, Message> {
    button(text(label).size(13))
        .padding([2, 8])
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: None,
                text_color: match chosen {
                    Some(true) => roles.accent_text,
                    Some(false) => roles.fg_subtle,
                    None => roles.fg,
                },
                border: Border::default().rounded(4).width(1).color(if hovered {
                    roles.border
                } else {
                    Color::TRANSPARENT
                }),
                ..button::Style::default()
            }
        })
        .on_press(message)
        .into()
}

fn value_text<'a>(value: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    container(text(value).size(13)).padding([2, 8]).into()
}

fn separator<'a>(mark: &'a str) -> Element<'a, Message> {
    container(text(mark).size(13).style(subtle))
        .padding([0, 2])
        .into()
}

fn hint<'a>(words: &'a str) -> Element<'a, Message> {
    container(text(words).size(12).style(subtle))
        .padding([0, 6])
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_kept_in_range() {
        let mut settings = Settings::default();
        apply(Change::TabSpaces(0), &mut settings);
        assert_eq!(settings.tab_spaces, 1);
        apply(Change::FontSize(100.0), &mut settings);
        assert_eq!(settings.font_size, 40.0);
    }
}
