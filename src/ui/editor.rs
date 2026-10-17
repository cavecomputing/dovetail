//! The text editing widget: the centred column, Tab as spaces, and XML-tag / {{macro}} colouring.

use std::ops::Range;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::keyboard::{Key, key::Named};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, Position, Status,
};
use iced::widget::{container, text_editor as editor_widget};
use iced::{Background, Border, Element, Fill, Font, Padding, Theme};

use crate::core::settings::{ReadingMode, Settings};
use crate::ui::app::Message;
use crate::ui::theme::{CHAR_WIDTH_RATIO, Roles};

pub const EDITOR_ID: &str = "doc";
pub const LINE_HEIGHT: f32 = 1.75;
pub const PADDING: Padding = Padding {
    top: 56.0,
    right: 24.0,
    bottom: 40.0,
    left: 24.0,
};

/// The editor in its column. Without `editable` the text is dimmed; it can still be scrolled,
/// selected and copied, and `App::update` drops any change to it.
pub fn view<'a>(content: &'a Content, settings: &Settings, editable: bool) -> Element<'a, Message> {
    let tab_spaces = usize::from(settings.tab_spaces);
    let editor = editor_widget(content)
        .id(EDITOR_ID)
        .key_binding(move |press| key_binding(press, tab_spaces))
        .wrapping(if settings.word_wrap {
            Wrapping::WordOrGlyph
        } else {
            Wrapping::None
        })
        .size(settings.font_size)
        .line_height(LINE_HEIGHT)
        .padding(PADDING)
        .height(Fill)
        .style(move |theme, _status| style(theme, editable))
        .on_action(Message::Edit);
    let editor: Element<'a, Message> = if settings.colour_markup {
        editor
            .highlight_with::<MarkupHighlighter>((), markup_format)
            .into()
    } else {
        editor.into()
    };

    let column = match settings.reading_mode {
        ReadingMode::Comfort { chars } => {
            let text_width = f32::from(chars) * settings.font_size * CHAR_WIDTH_RATIO;
            container(editor).max_width(text_width + PADDING.left + PADDING.right)
        }
        ReadingMode::FullWidth => container(editor),
    };
    container(column).center_x(Fill).height(Fill).into()
}

fn style(theme: &Theme, editable: bool) -> text_editor::Style {
    let roles = Roles::of(theme);
    text_editor::Style {
        background: Background::Color(iced::Color::TRANSPARENT),
        border: Border::default(),
        placeholder: roles.fg_subtle,
        value: if editable { roles.fg } else { roles.fg_muted },
        selection: roles.accent_cool.scale_alpha(0.35),
    }
}

/// Tab inserts spaces (never a tab character), Shift+Tab unindents, and the app's own
/// Ctrl shortcuts are left for the app.
fn key_binding(press: KeyPress, tab_spaces: usize) -> Option<Binding<Message>> {
    // A custom binding is used even when the editor isn't focused, so check first.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if press.modifiers.command() {
        return match press.key.as_ref() {
            Key::Character("p" | "0" | ",") => None,
            _ => Binding::from_key_press(press),
        };
    }
    match press.key.as_ref() {
        Key::Named(Named::Tab) if press.modifiers.shift() => {
            Some(Binding::Custom(Message::Unindent))
        }
        Key::Named(Named::Tab) => Some(Binding::Sequence(vec![Binding::Insert(' '); tab_spaces])),
        _ => Binding::from_key_press(press),
    }
}

/// Shift+Tab: removes up to one indent (`tab_spaces` spaces) from the start of the cursor's
/// line, or of every selected line, and keeps the cursor and selection on the same text.
pub fn unindent(content: &mut Content, tab_spaces: usize) {
    let cursor = content.cursor();
    let anchor = cursor.selection.unwrap_or(cursor.position);
    let first = cursor.position.line.min(anchor.line);
    let last = cursor.position.line.max(anchor.line);
    let mut removed = Vec::new();
    for line in first..=last {
        let text = content.line(line).map(|line| line.text.into_owned());
        let spaces = text.map_or(0, |text| {
            text.chars()
                .take(tab_spaces)
                .take_while(|c| *c == ' ')
                .count()
        });
        if spaces > 0 {
            // Select the spaces, then delete the selection.
            content.move_to(Cursor {
                position: Position {
                    line,
                    column: spaces,
                },
                selection: Some(Position { line, column: 0 }),
            });
            content.perform(Action::Edit(Edit::Delete));
        }
        removed.push(spaces);
    }
    let shifted = |position: Position| Position {
        line: position.line,
        column: position
            .column
            .saturating_sub(removed[position.line - first]),
    };
    content.move_to(Cursor {
        position: shifted(cursor.position),
        selection: cursor.selection.map(shifted),
    });
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Markup {
    Tag,
    Macro,
}

fn markup_format(markup: &Markup, theme: &Theme) -> highlighter::Format<Font> {
    let roles = Roles::of(theme);
    let color = match markup {
        Markup::Tag => roles.fg_muted,
        Markup::Macro => roles.accent_cool,
    };
    highlighter::Format {
        color: Some(color),
        font: None,
    }
}

/// Finds `<tags ...>` and `{{macros}}` within one line, as byte ranges.
pub fn markup_ranges(line: &str) -> Vec<(Range<usize>, Markup)> {
    let mut found = Vec::new();
    let mut from = 0;
    while from < line.len() {
        let rest = &line[from..];
        let next_tag = rest.match_indices('<').find(|(at, _)| {
            // A tag starts like `<a`, `</`, `<!` or `<?`, so "a < b" is left alone.
            let after = rest[at + 1..].chars().next();
            after.is_some_and(|c| c.is_alphabetic() || "/!?".contains(c))
        });
        let candidates = [
            next_tag.map(|(at, _)| (at, ">", Markup::Tag)),
            rest.find("{{").map(|at| (at, "}}", Markup::Macro)),
        ];
        let Some((start, close, markup)) = candidates.into_iter().flatten().min_by_key(|c| c.0)
        else {
            break;
        };
        match rest[start..].find(close) {
            Some(length) => {
                let end = start + length + close.len();
                found.push((from + start..from + end, markup));
                from += end;
            }
            None => from += start + 1,
        }
    }
    found
}

/// iced asks for colours one line at a time; `line` tracks how far it has got.
pub struct MarkupHighlighter {
    line: usize,
}

impl Highlighter for MarkupHighlighter {
    type Settings = ();
    type Highlight = Markup;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Markup)>;

    fn new(_settings: &()) -> Self {
        MarkupHighlighter { line: 0 }
    }

    fn update(&mut self, _settings: &()) {
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.line += 1;
        markup_ranges(line).into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(line: &str) -> Vec<(&str, Markup)> {
        markup_ranges(line)
            .into_iter()
            .map(|(range, markup)| (&line[range], markup))
            .collect()
    }

    #[test]
    fn finds_tags_and_macros() {
        assert_eq!(
            marked(r#"<system_instructions status="enabled">Hi {{char}}</system_instructions>"#),
            vec![
                (r#"<system_instructions status="enabled">"#, Markup::Tag),
                ("{{char}}", Markup::Macro),
                ("</system_instructions>", Markup::Tag),
            ]
        );
        assert_eq!(
            marked("{{#x}}…{{/x}}"),
            vec![("{{#x}}", Markup::Macro), ("{{/x}}", Markup::Macro)]
        );
    }

    #[test]
    fn plain_prose_is_left_alone() {
        assert!(marked("if a < b and c > d, it's fine").is_empty());
        assert!(marked("an unclosed <tag and {{macro").is_empty());
        assert!(marked("").is_empty());
    }

    #[test]
    fn unindent_removes_one_indent_of_tab_spaces() {
        let mut content: Content = Content::with_text("        two levels\n  x");
        content.move_to(Cursor {
            position: Position {
                line: 0,
                column: 10,
            },
            selection: None,
        });
        unindent(&mut content, 4);
        assert_eq!(content.text(), "    two levels\n  x");
        assert_eq!(content.cursor().position, Position { line: 0, column: 6 });

        // A selection unindents every line it touches, each by what it has (up to 4).
        content.move_to(Cursor {
            position: Position { line: 1, column: 3 },
            selection: Some(Position { line: 0, column: 4 }),
        });
        unindent(&mut content, 4);
        assert_eq!(content.text(), "two levels\nx");
    }
}
