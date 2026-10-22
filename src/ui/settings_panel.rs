//! The Settings panel (Ctrl+,), covering the text area and laid out like settings.toml. Every
//! change applies at once and is written to that file.

use iced::widget::TextInput;
use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Background, Border, Color, Element, Fill, Padding, Theme};

use crate::ai::key::KeyStore;
use crate::core::paths::display_path;
use crate::core::settings::{
    DEFAULT_BASE_URL, ReadingMode, RequestFormat, ScratchSyntax, Settings, Theme as ThemeSetting,
    WindowControls,
};
use crate::core::syntax::Token;
use crate::ui::app::{App, Message};
use crate::ui::assistant_actions::AssistantAction;
use crate::ui::editor;
use crate::ui::highlighter::{Highlight, highlight_format};
use crate::ui::status_bar::{bar, dot, glyph_button, keys, small};
use crate::ui::theme::{self, GLYPH_CLOSE, GLYPH_MENU, Roles, bold, icon_button};

/// The assistant's API key field. The key is saved as it changes, off the UI thread, since the
/// keyring may be slow; it never goes through `Message::Setting`.
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

pub fn view(app: &App) -> Element<'_, Message> {
    let settings = &app.settings;
    // The comfort width is remembered while full width is on, so switching back keeps it.
    let comfort_chars = match settings.reading_mode {
        ReadingMode::Comfort { chars } => chars,
        ReadingMode::FullWidth => app.last_comfort_chars,
    };
    let comfort = matches!(settings.reading_mode, ReadingMode::Comfort { .. });
    let comfort_with = |chars| {
        changed(settings, |s| {
            s.reading_mode = ReadingMode::Comfort { chars }
        })
    };

    let appearance = section(
        "appearance",
        vec![
            setting_row(
                "theme",
                pick(
                    settings,
                    settings.theme,
                    &[
                        ("dark", ThemeSetting::Dark),
                        ("light", ThemeSetting::Light),
                        ("system", ThemeSetting::System),
                    ],
                    |s, theme| s.theme = theme,
                ),
                comment(""),
            ),
            setting_row(
                "font",
                value_text("JetBrains Mono NL Nerd Font", Token::String),
                comment("bundled"),
            ),
            setting_row(
                "font_size",
                stepper(
                    settings.font_size.to_string(),
                    changed(settings, |s| s.font_size -= 1.0),
                    changed(settings, |s| s.font_size += 1.0),
                ),
                comment(format!(
                    "px, also {} and {}",
                    keys("Ctrl+="),
                    keys("Ctrl+-")
                )),
            ),
            setting_row(
                "reading_mode",
                row![
                    choices(vec![
                        (
                            "full width",
                            !comfort,
                            changed(settings, |s| s.reading_mode = ReadingMode::FullWidth)
                        ),
                        ("comfort", comfort, comfort_with(comfort_chars)),
                    ]),
                    stepper(
                        comfort_chars.to_string(),
                        comfort_with(comfort_chars.saturating_sub(5)),
                        comfort_with(comfort_chars + 5),
                    ),
                ]
                .spacing(CH)
                .into(),
                comment("chars a line, in comfort"),
            ),
        ],
    );

    let editor = section(
        "editor",
        vec![
            setting_row(
                "tab_spaces",
                stepper(
                    settings.tab_spaces.to_string(),
                    changed(settings, |s| s.tab_spaces = s.tab_spaces.saturating_sub(1)),
                    changed(settings, |s| s.tab_spaces = s.tab_spaces.saturating_add(1)),
                ),
                comment("spaces, never tabs; YAML follows the file, 2 if new"),
            ),
            setting_row(
                "word_wrap",
                on_off(settings, settings.word_wrap, |s, on| s.word_wrap = on),
                comment("off cuts long lines at the edge"),
            ),
            setting_row(
                "line_numbers",
                on_off(settings, settings.line_numbers, |s, on| s.line_numbers = on),
                comment(keys("Ctrl+Shift+L")),
            ),
            setting_row(
                "colour_markup",
                on_off(settings, settings.colour_markup, |s, on| {
                    s.colour_markup = on
                }),
                comment("XML tags and {{macros}}"),
            ),
            setting_row(
                "scratch_syntax",
                pick(
                    settings,
                    settings.scratch_syntax,
                    &[
                        ("markdown", ScratchSyntax::Markdown),
                        ("plain", ScratchSyntax::Plain),
                    ],
                    |s, syntax| s.scratch_syntax = syntax,
                ),
                comment("the rules scratch follows"),
            ),
            setting_row(
                "show_line_col",
                on_off(settings, settings.show_line_col, |s, on| {
                    s.show_line_col = on
                }),
                comment("in the status bar"),
            ),
        ],
    );

    let delay = settings.autosave_delay_ms;
    let saving = section(
        "saving",
        vec![setting_row(
            "autosave_delay_ms",
            stepper(
                delay.to_string(),
                changed(settings, |s| {
                    s.autosave_delay_ms = delay.saturating_sub(250)
                }),
                changed(settings, |s| s.autosave_delay_ms = delay + 250),
            ),
            comment("pause in typing"),
        )],
    );

    let format = settings.request_format;
    let assistant = section(
        "assistant",
        vec![
            setting_row(
                "base_url",
                field(settings, DEFAULT_BASE_URL, &settings.base_url, |s, url| {
                    s.base_url = url
                }),
                comment(""),
            ),
            setting_row(
                "model",
                field(
                    settings,
                    "as the server knows it",
                    &settings.model,
                    |s, model| s.model = model,
                ),
                comment(""),
            ),
            setting_row(
                "request_format",
                pick(
                    settings,
                    format,
                    &[
                        ("responses", RequestFormat::Responses),
                        ("chat completions", RequestFormat::ChatCompletions),
                    ],
                    |s, format| s.request_format = format,
                ),
                comment(""),
            ),
            setting_row(
                "api_key",
                key_input(&app.key_field.text),
                key_status(&app.key_field),
            ),
            setting_row(
                "",
                value_button(
                    "test connection".to_string(),
                    Look::Action,
                    Message::Assistant(AssistantAction::TestConnection),
                ),
                connection_result(app.connection.as_ref()),
            ),
        ],
    );

    let window = section(
        "window",
        vec![setting_row(
            "window_controls",
            pick(
                settings,
                settings.window_controls,
                &[
                    ("auto", WindowControls::Auto),
                    ("show", WindowControls::Show),
                    ("hide", WindowControls::Hide),
                ],
                |s, controls| s.window_controls = controls,
            ),
            comment("auto hides them on tiling WMs"),
        )],
    );

    let close = button(text(GLYPH_CLOSE).size(13).center())
        .width(28)
        .height(24)
        .padding(0)
        .style(icon_button)
        .on_press(Message::CloseMenu);
    // The first line, like the comment at the top of a file. In a narrow window the words are
    // cut off, not the close button.
    let header = row![
        container(comment(
            "every change applies and saves at once · Esc goes back"
        ))
        .width(Fill)
        .clip(true),
        close
    ]
    .align_y(iced::Center);

    // A blank line between sections, as in a file.
    let mut page = column![header, appearance, editor, saving, assistant].spacing(LINE);
    // macOS keeps its own window buttons, so there is nothing to choose.
    if !cfg!(target_os = "macos") {
        page = page.push(window);
    }
    page = page.push(shortcuts());
    // Laid out like settings.toml, in the editor's type, starting where the text does. Taller
    // than the window, it scrolls, with the bar at the window's right edge like the editor's.
    let page = container(page).padding(Padding {
        right: 12.0,
        ..editor::PADDING
    });
    // Settings takes the whole text area, edge to edge, over the text; Esc or the close button
    // goes back. `opaque` keeps clicks and scrolling off the text underneath.
    let page = container(scrollable(page).width(Fill).height(Fill))
        .width(Fill)
        .height(Fill)
        .style(|theme: &Theme| theme::fill(Roles::of(theme).bg_surface));
    iced::widget::opaque(page)
}

/// A keyboard shortcut: the keys as Linux and Windows write them, as macOS does where `keys()`
/// can't work it out ("" when it can; no first keys means it's macOS's alone), and what it does.
type Shortcut = (&'static str, &'static str, &'static str);

/// Every keyboard shortcut, by group.
const SHORTCUTS: [(&str, &[Shortcut]); 5] = [
    (
        "files",
        &[
            ("Ctrl+N", "", "new file"),
            ("Ctrl+O", "", "open a file"),
            ("→ / ←", "", "into a folder or up one, in Ctrl+O"),
            ("Ctrl+P", "", "scratch and the recent files"),
            ("Ctrl+0", "", "back to scratch"),
            ("Ctrl+S", "", "save now; in scratch, save to a file"),
            ("Ctrl+Shift+S", "", "save as"),
            (
                "Ctrl+W",
                "",
                "close the window, once the latest save is written",
            ),
            ("", "⌘Q", "close the window, as ⌘W does"),
            ("", "⌘M", "minimise the window"),
        ],
    ),
    (
        "view",
        &[
            ("Ctrl+L", "", "open or close the assistant pane"),
            (
                "Ctrl+Shift+P",
                "",
                "the command palette: the open file's tools",
            ),
            ("Ctrl+,", "", "open or close settings"),
            ("Ctrl+Shift+L", "", "line numbers on or off"),
            ("Ctrl+= / Ctrl+-", "", "bigger or smaller text"),
            (
                "Esc",
                "",
                "close what's open over the text and go back to it",
            ),
        ],
    ),
    (
        "find",
        &[
            ("Ctrl+F", "", "find"),
            ("Ctrl+Shift+F", "", "find and replace"),
            (
                "Enter / Shift+Enter",
                "",
                "next or previous match, in the find box",
            ),
            ("", "⌘G / ⇧⌘G", "next or previous match"),
            ("Ctrl+Enter", "", "replace every match, in the replace box"),
            ("Ctrl+G", "⌃G", "go to line"),
        ],
    ),
    (
        "editing",
        &[
            ("Tab / Shift+Tab", "", "indent or unindent, with spaces"),
            ("Shift+Enter", "", "new line, never a new list item"),
            (
                "Ctrl+D",
                "",
                "select the word, then the next place it appears",
            ),
            ("Ctrl+Z", "", "undo"),
            ("Ctrl+Shift+Z / Ctrl+Y", "", "redo"),
            ("Alt+↑ / Alt+↓", "⌥↑ / ⌥↓", "move the lines up or down"),
            ("Ctrl+Shift+D", "", "duplicate the lines"),
            ("Ctrl+Shift+K", "", "delete the lines"),
            (
                "Ctrl+Backspace / Ctrl+Delete",
                "⌥⌫ / ⌥⌦",
                "delete the word before or after",
            ),
            ("", "⌘⌫ / ⌘⌦", "delete to the start or end of the line"),
            ("", "⌘↑ / ⌘↓", "go to the start or end of the text"),
            (
                "Ctrl+/",
                "",
                "comment the lines out, in scripts and config files",
            ),
            ("Ctrl+B / Ctrl+I", "", "bold or italic, in Markdown"),
            (
                "Ctrl+Shift+Enter",
                "",
                "tick the lines' task boxes, in Markdown",
            ),
            ("Ctrl+A, C, X, V", "", "select all, copy, cut, paste"),
        ],
    ),
    (
        "assistant",
        &[
            (
                "Enter / Shift+Enter",
                "",
                "send, or a new line, in the assistant's box",
            ),
            (
                "Ctrl+Enter",
                "",
                "accept every suggestion, as one undo step",
            ),
            ("Ctrl+Backspace", "", "reject it"),
            ("Ctrl+Shift+Backspace", "", "reject every suggestion"),
        ],
    ),
];

/// The shortcuts section: every one of `SHORTCUTS` on this OS, the keys coloured as a key.
fn shortcuts<'a>() -> Element<'a, Message> {
    let mut rows = vec![comment("for reference; they can't be changed yet")];
    for (group, shortcuts) in SHORTCUTS {
        rows.push(space().height(LINE).into());
        rows.push(comment(group));
        for &(others, mac, what) in shortcuts {
            let shown = match (cfg!(target_os = "macos"), mac) {
                (true, "") => keys(others),
                (true, mac) => keys(mac),
                (false, _) if others.is_empty() => continue,
                (false, _) => others.to_string(),
            };
            let what = text(what).size(SIZE).line_height(editor::LINE_HEIGHT);
            rows.push(row![coloured(shown, Token::Key).width(30.0 * CH), what].into());
        }
    }
    section("shortcuts", rows)
}

/// The status bar while Settings is open: where the settings are saved, or why they can't be.
pub fn status(app: &App) -> Element<'_, Message> {
    let roles = Roles::of(&app.theme);
    let (dot_color, color, file) = match &app.settings_error {
        Some(reason) => (roles.accent_warm, roles.accent_warm, reason.clone()),
        None => (
            roles.accent_aqua,
            roles.fg_muted,
            display_path(&app.paths.settings_file()),
        ),
    };
    // The menu button keeps its place, unused, so the dot and path sit where the document's do.
    let left = row![
        glyph_button(GLYPH_MENU, None),
        dot(dot_color),
        small(file).color(color)
    ]
    .spacing(16)
    .align_y(iced::Center);
    bar(container(left).width(Fill).clip(true).into())
}

/// The text size and line height Settings is set in: the editor's, at its default size, so the
/// page reads like a file in the editor (and doesn't jump under the mouse as the font size
/// changes).
const SIZE: f32 = 14.0;
const LINE: f32 = SIZE * editor::LINE_HEIGHT;
/// One character's width, which the columns are counted in.
const CH: f32 = SIZE * theme::CHAR_WIDTH_RATIO;

/// A section: its name as a TOML table, then its settings, one a line.
fn section<'a>(name: &'a str, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let heading = text(format!("[{name}]"))
        .size(SIZE)
        .line_height(editor::LINE_HEIGHT)
        .font(bold())
        .style(|theme: &Theme| text::Style {
            color: Some(Roles::of(theme).accent_text),
        });
    column![heading, column(rows)].into()
}

/// One setting as a line of the file: its key, `=`, the value, and a comment, each starting in
/// its own column.
fn setting_row<'a>(
    key: &'a str,
    value: Element<'a, Message>,
    comment: Element<'a, Message>,
) -> Element<'a, Message> {
    let equals = if key.is_empty() { "" } else { "=" };
    row![
        coloured(key, Token::Key).width(19.0 * CH),
        coloured(equals, Token::Comment).width(2.0 * CH),
        container(value).width(34.0 * CH),
        comment,
    ]
    .align_y(iced::Center)
    .into()
}

/// `words` in the colour the editor gives `token` in a TOML file.
fn coloured<'a>(words: impl text::IntoFragment<'a>, token: Token) -> text::Text<'a> {
    text(words)
        .size(SIZE)
        .line_height(editor::LINE_HEIGHT)
        .style(move |theme: &Theme| text::Style {
            color: highlight_format(&Highlight::Data(token), theme).color,
        })
}

/// `# words`, after a setting or on a line of its own; nothing when `words` is empty. Long words
/// wrap onto the next line.
fn comment<'a>(words: impl Into<String>) -> Element<'a, Message> {
    let words = words.into();
    if words.is_empty() {
        return space().into();
    }
    coloured(format!("# {words}"), Token::Comment)
        .width(Fill)
        .into()
}

/// Options side by side; the chosen one is coloured as a TOML string.
fn choices<'a>(options: Vec<(&'a str, bool, Message)>) -> Element<'a, Message> {
    choices_as(options, Token::String)
}

fn choices_as<'a>(options: Vec<(&'a str, bool, Message)>, token: Token) -> Element<'a, Message> {
    let mut line = row![].spacing(CH).align_y(iced::Center);
    for (label, chosen, change) in options {
        let look = if chosen {
            Look::Chosen(token)
        } else {
            Look::Other
        };
        line = line.push(value_button(label.to_string(), look, change));
    }
    line.into()
}

/// One of a setting's values: `options` are each label and value, `current` is coloured as
/// chosen, and picking one sends `change` with its value.
fn pick<'a, T: Copy + PartialEq>(
    settings: &Settings,
    current: T,
    options: &[(&'a str, T)],
    change: fn(&mut Settings, T),
) -> Element<'a, Message> {
    let options = options
        .iter()
        .map(|&(label, value)| {
            (
                label,
                value == current,
                changed(settings, |s| change(s, value)),
            )
        })
        .collect();
    choices(options)
}

/// On and off, the chosen one coloured as a TOML `true` or `false`.
fn on_off<'a>(
    settings: &Settings,
    on: bool,
    change: fn(&mut Settings, bool),
) -> Element<'a, Message> {
    choices_as(
        vec![
            ("on", on, changed(settings, |s| change(s, true))),
            ("off", !on, changed(settings, |s| change(s, false))),
        ],
        Token::Literal,
    )
}

/// A number, coloured as one, with − and + on either side.
fn stepper<'a>(value: String, less: Message, more: Message) -> Element<'a, Message> {
    row![
        value_button("−".to_string(), Look::Other, less),
        value_text(value, Token::Number),
        value_button("+".to_string(), Look::Other, more),
    ]
    .align_y(iced::Center)
    .into()
}

/// A text box for a free-form setting, its text coloured as a TOML string; empty shows
/// `placeholder`.
fn field<'a>(
    settings: &Settings,
    placeholder: &str,
    value: &str,
    change: fn(&mut Settings, String),
) -> Element<'a, Message> {
    let settings = settings.clone();
    text_box(placeholder, value)
        .on_input(move |typed| changed(&settings, |s| change(s, typed)))
        .into()
}

/// The settings with one change made, as the message that applies them (`Message::Setting`).
fn changed(settings: &Settings, change: impl FnOnce(&mut Settings)) -> Message {
    let mut settings = settings.clone();
    change(&mut settings);
    Message::Setting(settings)
}

/// The API key, shown as dots. Each change is saved (see `KeyField`); empty deletes it.
fn key_input<'a>(key: &str) -> Element<'a, Message> {
    text_box("paste one if needed", key)
        .secure(true)
        .on_input(|typed| Message::Assistant(AssistantAction::KeyTyped(typed)))
        .into()
}

/// The text box `field` and `key_input` share, sized like a value in the file.
fn text_box<'a>(placeholder: &str, value: &str) -> TextInput<'a, Message> {
    text_input(placeholder, value)
        .size(SIZE)
        .line_height(editor::LINE_HEIGHT)
        .padding([0, 4])
        .width(32.0 * CH)
        .style(field_style)
}

/// The keyring by the name people know it by on this OS.
const KEYRING: &str = if cfg!(target_os = "macos") {
    "the Keychain"
} else if cfg!(windows) {
    "Credential Manager"
} else {
    "the system keyring"
};

/// Where the key is kept, in words: the keyring, or settings.toml on macOS or when there is none.
fn key_status<'a>(field: &KeyField) -> Element<'a, Message> {
    if let Some(error) = &field.error {
        return warning(error.clone());
    }
    let words = match (field.store, field.saving, field.text.trim().is_empty()) {
        (None, _, _) => "checking for a keyring…".to_string(),
        (_, true, _) => "saving…".to_string(),
        (Some(KeyStore::Keyring), _, false) => format!("stored in {KEYRING}"),
        (Some(KeyStore::Keyring), _, true) => format!("none saved · a key goes in {KEYRING}"),
        (Some(KeyStore::SettingsFile), _, false) => "stored in settings.toml".to_string(),
        (Some(KeyStore::SettingsFile), _, true) => {
            "none saved · a key goes in settings.toml".to_string()
        }
    };
    comment(words)
}

/// The test's outcome after the button: what the model replied, or why it failed.
fn connection_result<'a>(connection: Option<&Connection>) -> Element<'a, Message> {
    match connection {
        None => space().into(),
        Some(Connection::Testing) => comment("testing…"),
        Some(Connection::Passed(reply)) if reply.trim().is_empty() => {
            ok_line("ok · it answered".to_string())
        }
        Some(Connection::Passed(reply)) => {
            let reply: String = reply.trim().chars().take(60).collect();
            ok_line(format!("ok · it replied \u{201c}{reply}\u{201d}"))
        }
        Some(Connection::Failed(reason)) => warning(reason.clone()),
    }
}

/// A comment saying something worked, in the success colour.
fn ok_line<'a>(words: String) -> Element<'a, Message> {
    coloured_comment(words, |roles| roles.accent_aqua)
}

/// A problem in words, in the warm accent (never red).
fn warning<'a>(words: String) -> Element<'a, Message> {
    coloured_comment(words, |roles| roles.accent_warm)
}

fn coloured_comment<'a>(words: String, color: fn(&Roles) -> Color) -> Element<'a, Message> {
    text(format!("# {words}"))
        .size(SIZE)
        .line_height(editor::LINE_HEIGHT)
        .width(Fill)
        .style(move |theme: &Theme| text::Style {
            color: Some(color(Roles::of(theme))),
        })
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
        value: roles.accent_aqua,
        selection: roles.accent_cool.scale_alpha(0.35),
    }
}

/// How a clickable value is drawn.
#[derive(Clone, Copy)]
enum Look {
    /// The setting's value, in the colour TOML gives this kind of value.
    Chosen(Token),
    /// Another choice, or − and +: dim until picked.
    Other,
    /// Something to do rather than a value.
    Action,
}

/// A clickable value, framed while hovered.
fn value_button<'a>(label: String, look: Look, message: Message) -> Element<'a, Message> {
    button(text(label).size(SIZE).line_height(editor::LINE_HEIGHT))
        .padding([0, 4])
        .style(move |theme: &Theme, status| {
            let roles = Roles::of(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let text_color = match look {
                Look::Chosen(token) => highlight_format(&Highlight::Data(token), theme)
                    .color
                    .unwrap_or(roles.fg),
                Look::Other if hovered => roles.fg_muted,
                Look::Other => roles.fg_subtle,
                Look::Action => roles.fg,
            };
            button::Style {
                background: None,
                text_color,
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

/// A value that isn't clicked, lined up with the buttons' text.
fn value_text<'a>(value: impl text::IntoFragment<'a>, token: Token) -> Element<'a, Message> {
    container(coloured(value, token)).padding([0, 4]).into()
}
