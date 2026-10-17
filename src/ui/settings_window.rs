//! The Settings window (Ctrl+,). Every change applies at once and is written to settings.toml.

use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Background, Border, Color, Element, Fill, Theme};

use crate::core::paths::display_path;
use crate::core::settings::{ReadingMode, Settings, Theme as ThemeSetting, WindowControls};
use crate::ui::app::{App, Message};
use crate::ui::status_bar::{self, dot, small};
use crate::ui::theme::{self, Roles, subtle};

/// One edit made in the Settings window.
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
    ProjectsFolder(String),
    ClaudeModel(String),
    WindowControls(WindowControls),
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
        Change::ProjectsFolder(folder) if folder.trim().is_empty() => {
            settings.projects_folder = None
        }
        Change::ProjectsFolder(folder) => settings.projects_folder = Some(folder.into()),
        Change::ClaudeModel(model) => settings.claude_model = model,
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
                row![value_text("JetBrainsMono Nerd Font"), hint("bundled")]
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
                        ("comfort", comfort, comfort_with(comfort_chars)),
                        (
                            "full width",
                            !comfort,
                            Change::ReadingMode(ReadingMode::FullWidth)
                        ),
                    ]),
                    separator("·"),
                    stepper(
                        comfort_chars.to_string(),
                        "chars",
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
    let projects_folder = match &settings.projects_folder {
        Some(folder) => folder.display().to_string(),
        None => String::new(),
    };
    let saving = section(
        "saving",
        vec![
            setting_row(
                "Autosave after",
                stepper(
                    delay.to_string(),
                    "ms pause in typing",
                    Change::AutosaveDelay(delay.saturating_sub(250)),
                    Change::AutosaveDelay(delay + 250),
                ),
            ),
            setting_row(
                "Projects folder",
                field(
                    &display_path(&app.paths.projects_dir),
                    &projects_folder,
                    Change::ProjectsFolder,
                ),
            ),
        ],
    );

    let claude = section(
        "claude",
        vec![
            setting_row("API key", hint("arrives in phase 5, stored in the keyring")),
            setting_row(
                "Model",
                field("model id", &settings.claude_model, Change::ClaudeModel),
            ),
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

    let sections = column![appearance, editor, saving, claude, window]
        .spacing(28)
        .max_width(640);
    let page = scrollable(container(sections).center_x(Fill).padding([40, 24])).height(Fill);

    let roles = Roles::of(&app.theme);
    let status: Element<'_, Message> = match &app.settings_error {
        Some(reason) => row![
            dot(roles.accent_warm),
            small(reason.as_str()).color(roles.accent_warm)
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into(),
        None => row![
            dot(roles.accent_aqua),
            small(display_path(&app.paths.settings_file()))
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into(),
    };
    let footer = status_bar::bar(
        row![
            status,
            space::horizontal(),
            // Hidden while there's a problem, which needs the room and isn't instant.
            small(if app.settings_error.is_some() {
                ""
            } else {
                "changes apply instantly"
            })
        ]
        .align_y(iced::Center)
        .padding([0, 6])
        .into(),
    );
    column![page, footer].into()
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
        line = line.push(value_button(label.to_string(), Some(chosen), change));
    }
    line.into()
}

fn on_off<'a>(on: bool, change: fn(bool) -> Change) -> Element<'a, Message> {
    choices(vec![("on", on, change(true)), ("off", !on, change(false))])
}

/// A number with − and + on either side, and its unit after.
fn stepper<'a>(value: String, unit: &'a str, less: Change, more: Change) -> Element<'a, Message> {
    row![
        value_button("−".to_string(), None, less),
        value_text(value),
        value_button("+".to_string(), None, more),
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
        .style(|theme: &Theme, status| {
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
        })
        .into()
}

/// A clickable value. `chosen` colours it as an option: yellow if picked, dim if not.
fn value_button<'a>(label: String, chosen: Option<bool>, change: Change) -> Element<'a, Message> {
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
        .on_press(Message::Setting(change))
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

    #[test]
    fn empty_projects_folder_means_the_default() {
        let mut settings = Settings::default();
        apply(Change::ProjectsFolder("~/Writing".into()), &mut settings);
        assert!(settings.projects_folder.is_some());
        apply(Change::ProjectsFolder("  ".into()), &mut settings);
        assert_eq!(settings.projects_folder, None);
    }
}
