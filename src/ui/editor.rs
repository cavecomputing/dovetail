//! The text editing widget: the centred column, Tab as spaces, undo and redo, and XML-tag /
//! {{macro}} colouring. A `Pane` pairs the widget's content with the text model behind it.

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::keyboard::{Key, key::Named};
use iced::widget::container;
use iced::widget::text::Wrapping;
use iced::{Background, Border, Element, Fill, Font, Padding, Theme};

use crate::core::settings::{ReadingMode, Settings};
use crate::core::text_file::newlines_only;
use crate::core::text_model::{Change, TextModel, offset, position};
use crate::ui::app::Message;
use crate::ui::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, Position, Status, TextEditor,
};
use crate::ui::theme::{CHAR_WIDTH_RATIO, Roles};

pub const EDITOR_ID: &str = "doc";
/// The left pane of a project: a committed version, read-only.
pub const REFERENCE_ID: &str = "reference";
pub const LINE_HEIGHT: f32 = 1.75;
pub const PADDING: Padding = Padding {
    top: 56.0,
    right: 24.0,
    bottom: 40.0,
    left: 24.0,
};

/// The editor in its column; `on_action` wraps what it reports. Without `editable` the text is
/// dimmed; it can still be scrolled, selected and copied, and `App::update` drops any change to
/// it.
pub fn view<'a>(
    content: &'a Content,
    settings: &Settings,
    editable: bool,
    id: &'static str,
    on_action: fn(Action) -> Message,
    padding: Padding,
) -> Element<'a, Message> {
    let tab_spaces = usize::from(settings.tab_spaces);
    let editor = TextEditor::new(content)
        .id(id)
        .key_binding(move |press| key_binding(press, tab_spaces, editable))
        .wrapping(if settings.word_wrap {
            Wrapping::WordOrGlyph
        } else {
            Wrapping::None
        })
        .size(settings.font_size)
        .line_height(LINE_HEIGHT)
        .padding(padding)
        .height(Fill)
        .style(move |theme, _status| style(theme, editable))
        .on_action(on_action);
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
            container(editor).max_width(text_width + padding.left + padding.right)
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

/// Tab inserts spaces (never a tab character), Shift+Tab unindents, Ctrl+Z undoes, Ctrl+Shift+Z
/// and Ctrl+Y redo, and the app's own Ctrl shortcuts are left for the app. A pane that isn't
/// `editable` keeps only iced's own keys (moving, selecting, copying).
fn key_binding(press: KeyPress, tab_spaces: usize, editable: bool) -> Option<Binding<Message>> {
    // A custom binding is used even when the editor isn't focused, so check first.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if press.modifiers.command() && matches!(press.key.as_ref(), Key::Character("p" | "0" | ",")) {
        return None;
    }
    if !editable {
        return Binding::from_key_press(press);
    }
    if press.modifiers.command() {
        return match press.key.as_ref() {
            Key::Character("z" | "Z") if press.modifiers.shift() => {
                Some(Binding::Custom(Message::Redo))
            }
            Key::Character("z" | "Z") => Some(Binding::Custom(Message::Undo)),
            Key::Character("y" | "Y") => Some(Binding::Custom(Message::Redo)),
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

/// One editor pane: what the widget shows, and the text model with its undo history. Every
/// change to the text goes through here, so the two always hold the same text.
pub struct Pane {
    pub content: Content,
    pub model: TextModel,
}

impl Pane {
    pub fn new(text: &str) -> Pane {
        Pane {
            content: Content::with_text(text),
            model: TextModel::new(text.to_string()),
        }
    }

    /// Does what the widget asked. An edit is recorded for undo; returns whether the text
    /// changed.
    pub fn perform(&mut self, action: Action) -> bool {
        if !action.is_edit() {
            self.content.perform(action);
            return false;
        }
        // Pasted text may hold `\r` line endings, which the model's line arithmetic doesn't know.
        let action = match action {
            Action::Edit(Edit::Paste(pasted)) if pasted.contains('\r') => {
                Action::Edit(Edit::Paste(Arc::new(newlines_only(&pasted))))
            }
            other => other,
        };
        let cursor = self.content.cursor();
        let anchor = cursor.selection.unwrap_or(cursor.position);
        let text = self.model.text();
        let mut edit_start = offset(text, cursor.position.line, cursor.position.column)
            .min(offset(text, anchor.line, anchor.column));
        // Backspace with nothing selected deletes the character before the cursor.
        if matches!(action, Action::Edit(Edit::Backspace)) && cursor.selection.is_none() {
            edit_start -= text[..edit_start]
                .chars()
                .next_back()
                .map_or(0, char::len_utf8);
        }
        self.content.perform(action);
        let changed = self
            .model
            .record(self.content.text(), edit_start, Instant::now())
            .is_some();
        debug_assert_eq!(self.model.text(), self.content.text());
        changed
    }

    /// Ctrl+Z. Returns whether the text changed.
    pub fn undo(&mut self) -> bool {
        let changes = self.model.undo();
        self.replay(&changes)
    }

    /// Ctrl+Shift+Z or Ctrl+Y. Returns whether the text changed.
    pub fn redo(&mut self) -> bool {
        let changes = self.model.redo();
        self.replay(&changes)
    }

    /// Replaces `range` (byte offsets) with `text` as one undo step, such as the Rewrite clear.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let change = self.model.replace(range, text);
        self.replay(&[change]);
    }

    /// Shift+Tab: removes up to one indent (`tab_spaces` spaces) from the start of the cursor's
    /// line, or of every selected line, and keeps the cursor and selection on the same text.
    /// Returns whether the text changed.
    pub fn unindent(&mut self, tab_spaces: usize) -> bool {
        let cursor = self.content.cursor();
        let anchor = cursor.selection.unwrap_or(cursor.position);
        let first = cursor.position.line.min(anchor.line);
        let last = cursor.position.line.max(anchor.line);
        let changes = self.model.unindent(first..=last, tab_spaces);
        if !self.replay(&changes) {
            return false;
        }
        // Each change removes spaces at the start of one line; move the cursor back by as many.
        let text = self.model.text();
        let shifted = |at: Position| {
            let removed = changes
                .iter()
                .find(|change| position(text, change.at).0 == at.line)
                .map_or(0, |change| change.removed.len());
            Position {
                line: at.line,
                column: at.column.saturating_sub(removed),
            }
        };
        self.content.move_to(Cursor {
            position: shifted(cursor.position),
            selection: cursor.selection.map(shifted),
        });
        true
    }

    /// Copies changes the model made into the widget, in order: select what was removed, then
    /// paste over it (or delete it). This keeps the scroll position and leaves the caret at the
    /// end of the last change. Returns whether there were any.
    fn replay(&mut self, changes: &[Change]) -> bool {
        let mut text = self.content.text();
        for change in changes {
            let end = change.at + change.removed.len();
            let (line, column) = position(&text, change.at);
            let (end_line, end_column) = position(&text, end);
            self.content.move_to(Cursor {
                position: Position {
                    line: end_line,
                    column: end_column,
                },
                selection: Some(Position { line, column }),
            });
            self.content
                .perform(Action::Edit(if change.inserted.is_empty() {
                    Edit::Delete
                } else {
                    Edit::Paste(Arc::new(change.inserted.clone()))
                }));
            text.replace_range(change.at..end, &change.inserted);
        }
        debug_assert_eq!(self.model.text(), self.content.text());
        !changes.is_empty()
    }
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

    fn cursor_at(line: usize, column: usize) -> Cursor {
        Cursor {
            position: Position { line, column },
            selection: None,
        }
    }

    #[test]
    fn edits_are_recorded_and_undo_redo_replay_into_the_widget() {
        let mut pane = Pane::new("hello world");
        pane.content.move_to(cursor_at(0, 5));
        for c in ",there".chars() {
            assert!(pane.perform(Action::Edit(Edit::Insert(c))));
        }
        pane.perform(Action::Edit(Edit::Backspace));
        assert_eq!(pane.content.text(), "hello,ther world");
        // The Backspace is one step and the typed word another.
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "hello,there world");
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "hello world");
        assert_eq!(pane.model.text(), "hello world");
        assert!(pane.redo() && pane.redo());
        assert_eq!(pane.content.text(), "hello,ther world");
        assert!(!pane.redo(), "nothing left to redo");
    }

    #[test]
    fn pasting_over_a_selection_and_undoing_it() {
        let mut pane = Pane::new("one\ntwo\nthree");
        pane.content.move_to(Cursor {
            position: Position { line: 2, column: 2 },
            selection: Some(Position { line: 0, column: 1 }),
        });
        pane.perform(Action::Edit(Edit::Paste(Arc::new("X\nY".to_string()))));
        assert_eq!(pane.content.text(), "oX\nYree");
        pane.undo();
        assert_eq!(pane.content.text(), "one\ntwo\nthree");
    }

    #[test]
    fn pasted_carriage_returns_keep_the_widget_and_model_in_step() {
        let mut pane = Pane::new("");
        pane.perform(Action::Edit(Edit::Paste(Arc::new(
            "a\rb\n\réé".to_string(),
        ))));
        assert_eq!(pane.content.text(), "a\nb\n\néé");
        pane.content.move_to(cursor_at(1, 1));
        assert!(pane.perform(Action::Edit(Edit::Backspace)));
        assert_eq!(pane.content.text(), "a\n\n\néé");
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "a\nb\n\néé");
        pane.replace(0..pane.model.text().len(), "x\ny");
        assert_eq!(pane.content.text(), "x\ny");
    }

    #[test]
    fn unindent_removes_one_indent_of_tab_spaces() {
        let mut pane = Pane::new("        two levels\n  x");
        pane.content.move_to(cursor_at(0, 10));
        assert!(pane.unindent(4));
        assert_eq!(pane.content.text(), "    two levels\n  x");
        assert_eq!(
            pane.content.cursor().position,
            Position { line: 0, column: 6 }
        );

        // A selection unindents every line it touches, each by what it has (up to 4).
        pane.content.move_to(Cursor {
            position: Position { line: 1, column: 3 },
            selection: Some(Position { line: 0, column: 4 }),
        });
        assert!(pane.unindent(4));
        assert_eq!(pane.content.text(), "two levels\nx");
        assert_eq!(
            pane.content.cursor().position,
            Position { line: 1, column: 1 }
        );

        // One undo puts both lines back.
        pane.undo();
        assert_eq!(pane.content.text(), "    two levels\n  x");
    }
}
