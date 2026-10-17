//! The text editing widget: the centred column, Tab as spaces, undo and redo, and XML-tag /
//! {{macro}} colouring. A `Pane` pairs the widget's content with the text model behind it.

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::keyboard::{Key, key::Named};
use iced::widget::container;
use iced::widget::text::Wrapping;
use iced::widget::text_editor::Motion;
use iced::{Background, Border, Element, Fill, Font, Padding, Theme};

use crate::core::markup::{Markup, markup_ranges};
use crate::core::settings::{ReadingMode, Settings};
use crate::core::text_file::newlines_only;
use crate::core::text_model::{Change, TextModel, offset, position};
use crate::ui::app::Message;
use crate::ui::idea_actions::IdeaAction;
use crate::ui::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, PaintRun, Position, Status, TextEditor,
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

/// The editor in its column, with `paint` drawn under the text; `on_action` wraps what it
/// reports. The draft (`EDITOR_ID`) also reports where its caret is drawn, for the feeling-note
/// card. Without `editable` the text is dimmed; it can still be scrolled, selected and copied,
/// and `App::update` drops any change to it.
pub fn view<'a>(
    content: &'a Content,
    settings: &Settings,
    editable: bool,
    id: &'static str,
    on_action: fn(Action) -> Message,
    padding: Padding,
    paint: &'a [PaintRun],
) -> Element<'a, Message> {
    let tab_spaces = usize::from(settings.tab_spaces);
    let mut editor = TextEditor::new(content)
        .id(id)
        .key_binding(move |press| key_binding(press, tab_spaces, editable, id))
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
        .on_action(on_action)
        .paint(paint);
    if id == EDITOR_ID {
        editor = editor.on_caret(|caret, area| Message::Idea(IdeaAction::Caret(caret, area)));
    }
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
/// and Ctrl+Y redo, Ctrl+X and Ctrl+C also keep the selection's paint (`Message::Clip`), and the
/// app's own Ctrl shortcuts are left for the app. A pane that isn't `editable` keeps only iced's
/// own keys (moving, selecting, copying). `id` is the editor's.
fn key_binding(
    press: KeyPress,
    tab_spaces: usize,
    editable: bool,
    id: &'static str,
) -> Option<Binding<Message>> {
    // A custom binding is used even when the editor isn't focused, so check first.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if press.modifiers.command()
        && matches!(
            press.key.as_ref(),
            Key::Character("p" | "0" | "," | "i" | "e" | "w" | "W")
        )
    {
        return None;
    }
    if let Some(cut_or_copy @ (Binding::Cut | Binding::Copy)) =
        Binding::from_key_press(press.clone())
    {
        let clip = Binding::Custom(Message::Clip(id));
        return Some(Binding::Sequence(vec![clip, cut_or_copy]));
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

    /// The selected text as a byte range, or `None` when nothing is selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        let cursor = self.content.cursor();
        let anchor = cursor.selection?;
        let text = self.model.text();
        let a = offset(text, cursor.position.line, cursor.position.column);
        let b = offset(text, anchor.line, anchor.column);
        (a != b).then(|| a.min(b)..a.max(b))
    }

    /// Drops the selection, leaving the caret where it is, so the view doesn't scroll.
    /// (`move_to` can't: it keeps any selection when given none.)
    pub fn deselect(&mut self) {
        let cursor = self.content.cursor();
        if let Some(anchor) = cursor.selection {
            // A move with a selection goes to the selection's edge on that side: the caret's own.
            let before =
                (cursor.position.line, cursor.position.column) < (anchor.line, anchor.column);
            let motion = if before { Motion::Left } else { Motion::Right };
            self.content.perform(Action::Move(motion));
        }
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
    fn selection_is_a_byte_range_and_deselect_drops_it() {
        let mut pane = Pane::new("café au lait");
        pane.content.move_to(Cursor {
            position: Position { line: 0, column: 2 },
            selection: Some(Position { line: 0, column: 9 }),
        });
        assert_eq!(pane.selection(), Some(2..9));
        pane.deselect();
        assert_eq!(pane.selection(), None);
        assert_eq!(
            pane.content.cursor().position.column,
            2,
            "the caret stays where it was"
        );
        pane.deselect();
        assert_eq!(
            pane.content.cursor().position.column,
            2,
            "nothing selected: no move"
        );
        // Selected the other way, the caret is at the end, and stays there.
        pane.content.move_to(Cursor {
            position: Position { line: 0, column: 9 },
            selection: Some(Position { line: 0, column: 2 }),
        });
        pane.deselect();
        assert_eq!(pane.selection(), None);
        assert_eq!(pane.content.cursor().position.column, 9);
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
