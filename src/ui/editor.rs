//! The text editing widget: the text column, Tab as spaces, undo and redo, and XML-tag /
//! {{macro}} colouring. A `Pane` pairs the widget's content with the text model behind it, and
//! shows suggested edits inside the text until each is accepted or rejected (review mode).

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::keyboard::{Key, key::Named};
use iced::widget::container;
use iced::widget::text::Wrapping;
use iced::widget::text_editor::Motion;
use iced::{Background, Border, Color, Element, Fill, Font, Padding, Theme};

use crate::core::markup::{Markup, markup_ranges};
use crate::core::review::{DisplayMap, Review, Shown, Suggestion};
use crate::core::settings::{ReadingMode, Settings};
use crate::core::text_file::newlines_only;
use crate::core::text_model::{Change, TextModel, offset, position};
use crate::project::paint::{IdeaId, Span};
use crate::ui::app::Message;
use crate::ui::idea_actions::IdeaAction;
use crate::ui::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, PaintLook, PaintRun, Position, Status,
    TextEditor,
};
use crate::ui::theme::{CHAR_WIDTH_RATIO, MONO, Roles};

pub const EDITOR_ID: &str = "doc";
/// The left pane of a project: a committed version, read-only.
pub const REFERENCE_ID: &str = "reference";
pub const LINE_HEIGHT: f32 = 1.5;
/// Text starts near the top left, like any text editor, with just enough room to breathe.
pub const PADDING: Padding = Padding {
    top: 12.0,
    right: 16.0,
    bottom: 24.0,
    left: 16.0,
};

/// The pane's editor in its column, with `paint` (and any suggestions, from `Pane::suggestion_runs`)
/// drawn under the text; `on_action` wraps what it reports. The draft (`EDITOR_ID`) also reports
/// where its caret is drawn, for the feeling-note card. Without `editable` the text is dimmed; it
/// can still be scrolled, selected and copied, and `App::update` drops any change to it.
pub fn view<'a>(
    pane: &'a Pane,
    settings: &Settings,
    editable: bool,
    id: &'static str,
    on_action: fn(Action) -> Message,
    padding: Padding,
    paint: Vec<PaintRun>,
) -> Element<'a, Message> {
    let tab_spaces = usize::from(settings.tab_spaces);
    let reviewing = pane.reviewing();
    let colouring = Colouring {
        markup: settings.colour_markup,
        review: pane.review_highlights(),
    };
    let mut editor = TextEditor::new(&pane.content)
        .id(id)
        .key_binding(move |press| key_binding(press, tab_spaces, editable, reviewing, id))
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
        .paint(paint)
        .on_decide(|id, accept| {
            Message::Decide(if accept {
                Decision::Accept(Some(id))
            } else {
                Decision::Reject(Some(id))
            })
        })
        .highlight_with::<MarkupHighlighter>(colouring, highlight_format);
    if id == EDITOR_ID {
        editor = editor.on_caret(|caret, area| Message::Idea(IdeaAction::Caret(caret, area)));
    }

    let column = match settings.reading_mode {
        ReadingMode::Comfort { chars } => {
            let text_width = f32::from(chars) * settings.font_size * CHAR_WIDTH_RATIO;
            container(editor).max_width(text_width + padding.left + padding.right)
        }
        ReadingMode::FullWidth => container(editor),
    };
    container(column).width(Fill).height(Fill).into()
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
/// own keys (moving, selecting, copying). While `reviewing`, Tab accepts the suggestion at the
/// caret and Esc rejects it (typing is off, see `Pane::perform`), and Ctrl+C and Ctrl+X copy
/// through `Message::Clip` alone, since the widget's text has the suggestions in it. `id` is the
/// editor's.
fn key_binding(
    press: KeyPress,
    tab_spaces: usize,
    editable: bool,
    reviewing: bool,
    id: &'static str,
) -> Option<Binding<Message>> {
    // A custom binding is used even when the editor isn't focused, so check first.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if press.modifiers.command()
        && matches!(
            press.key.as_ref(),
            Key::Character("p" | "0" | "," | "i" | "e" | "l" | "w" | "W")
        )
    {
        return None;
    }
    if let Some(cut_or_copy @ (Binding::Cut | Binding::Copy)) =
        Binding::from_key_press(press.clone())
    {
        let clip = Binding::Custom(Message::Clip(id));
        return match reviewing {
            true => Some(clip),
            false => Some(Binding::Sequence(vec![clip, cut_or_copy])),
        };
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
        Key::Named(Named::Tab) if reviewing && !press.modifiers.shift() => {
            Some(Binding::Custom(Message::Decide(Decision::Accept(None))))
        }
        Key::Named(Named::Escape) if reviewing => {
            Some(Binding::Custom(Message::Decide(Decision::Reject(None))))
        }
        Key::Named(Named::Tab) if press.modifiers.shift() => {
            Some(Binding::Custom(Message::Unindent))
        }
        Key::Named(Named::Tab) => Some(Binding::Sequence(vec![Binding::Insert(' '); tab_spaces])),
        _ => Binding::from_key_press(press),
    }
}

/// What to do with suggested edits: accept or reject one (by id; `None` is the one at or after
/// the caret), or all of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Accept(Option<u64>),
    Reject(Option<u64>),
    AcceptAll,
    RejectAll,
}

/// One editor pane: what the widget shows, and the text model with its undo history. Every
/// change to the text goes through here, so the two always hold the same text, except in review
/// mode, when the widget also shows the suggestions waiting in `review`.
pub struct Pane {
    pub content: Content,
    pub model: TextModel,
    review: Review,
    /// While any suggestions wait: the text with them spliced in, which the widget shows, and
    /// how it maps back to the text. Typing is off meanwhile.
    display: Option<DisplayMap>,
}

impl Pane {
    pub fn new(text: &str) -> Pane {
        Pane {
            content: Content::with_text(text),
            model: TextModel::new(text.to_string()),
            review: Review::default(),
            display: None,
        }
    }

    /// Whether suggestions are waiting (review mode). Typing is off until they're all decided;
    /// moving, selecting, copying, undo and redo still work.
    pub fn reviewing(&self) -> bool {
        self.display.is_some()
    }

    /// The suggestions waiting, in text order.
    pub fn suggestions(&self) -> &[Suggestion] {
        self.review.suggestions()
    }

    /// Shows a suggestion to put `text` in place of `range` (byte offsets into the model's
    /// text), in `idea`'s colour when it names one. The caret stays where it is. Returns its
    /// id, or why it was refused.
    pub fn suggest(
        &mut self,
        range: Range<usize>,
        text: &str,
        idea: Option<IdeaId>,
    ) -> Result<u64, String> {
        let caret = self.to_text(self.content.cursor().position);
        let id = self
            .review
            .add(self.model.text(), range, &newlines_only(text), idea)?;
        self.show_review();
        self.put_caret(caret);
        Ok(id)
    }

    /// Accepts or rejects suggestions. Accepting puts the new text in as one undo step, painted
    /// with the suggestion's idea when it names one, and leaves the caret after it; rejecting
    /// leaves the caret where it was. Returns whether the text changed.
    pub fn decide(&mut self, decision: Decision) -> bool {
        let caret = self.content.cursor().position;
        let at_caret = self.display.as_ref().and_then(|display| {
            display.suggestion_at(offset(&display.text, caret.line, caret.column))
        });
        let caret = self.to_text(caret);
        let chosen: Vec<Suggestion> = match decision {
            Decision::Accept(id) | Decision::Reject(id) => id
                .or(at_caret)
                .and_then(|id| self.review.take(id))
                .into_iter()
                .collect(),
            Decision::AcceptAll | Decision::RejectAll => self.review.take_all(),
        };
        let changes = match decision {
            Decision::Accept(_) | Decision::AcceptAll => self.model.accept(&chosen),
            Decision::Reject(_) | Decision::RejectAll => Vec::new(),
        };
        let changed = self.replay(&changes);
        if !changed {
            self.put_caret(caret);
        }
        changed
    }

    /// Where the caret is, as a byte offset of the model's text.
    pub fn caret(&self) -> usize {
        self.to_text(self.content.cursor().position)
    }

    /// Where widget position `at` is in the model's text. Inside a suggestion's ghost text or
    /// buttons, that's where the suggestion sits.
    fn to_text(&self, at: Position) -> usize {
        let shown_offset = offset(self.shown(), at.line, at.column);
        match &self.display {
            Some(display) => display.to_text(shown_offset),
            None => shown_offset,
        }
    }

    /// Puts the caret at byte `offset` of the model's text, with nothing selected.
    fn put_caret(&mut self, offset: usize) {
        let position = self.position(offset, true);
        self.content.move_to(Cursor {
            position,
            selection: None,
        });
    }

    /// Where byte `offset` of the model's text is in the widget. In review mode an offset right
    /// at a suggestion's ghost text goes before it, or after its buttons when `after` is set.
    pub fn position(&self, offset: usize, after: bool) -> Position {
        let shown_offset = match &self.display {
            Some(display) => display.to_display(offset, after),
            None => offset,
        };
        let (line, column) = position(self.shown(), shown_offset);
        Position { line, column }
    }

    /// The suggestions as the editor draws them: the replaced text struck through, the new text
    /// as ghost text, and room for the buttons, in their idea's colour (`idea_color`) or the
    /// cool accent. With `stripe` (in a project) each also gets a pending stripe in the gutter.
    pub fn suggestion_runs(
        &self,
        roles: &Roles,
        idea_color: impl Fn(IdeaId) -> Option<Color>,
        stripe: bool,
    ) -> Vec<PaintRun> {
        let Some(display) = &self.display else {
            return Vec::new();
        };
        let at = |offset| {
            let (line, column) = position(&display.text, offset);
            Position { line, column }
        };
        let mut runs = Vec::new();
        for piece in display.pieces.iter().filter(|p| !p.display.is_empty()) {
            let Some(suggestion) = self
                .suggestions()
                .iter()
                .find(|s| Some(s.id) == piece.suggestion)
            else {
                continue;
            };
            let look = match piece.shown {
                Shown::Text => continue,
                Shown::Struck => PaintLook::Struck(roles.fg_subtle),
                Shown::Ghost => PaintLook::Ghost,
                Shown::Buttons => PaintLook::Buttons {
                    id: suggestion.id,
                    stripe,
                },
            };
            runs.push(PaintRun {
                start: at(piece.display.start),
                end: at(piece.display.end),
                color: suggestion
                    .idea
                    .and_then(&idea_color)
                    .unwrap_or(roles.accent_cool),
                look,
            });
        }
        runs
    }

    /// The struck and ghost text and the button room, one entry per line, for the highlighter.
    fn review_highlights(&self) -> Vec<(usize, Range<usize>, Highlight)> {
        let Some(display) = &self.display else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for piece in &display.pieces {
            let highlight = match piece.shown {
                Shown::Struck => Highlight::Struck,
                Shown::Ghost => Highlight::Ghost,
                Shown::Buttons => Highlight::Hidden,
                Shown::Text => continue,
            };
            let (mut line, mut column) = position(&display.text, piece.display.start);
            for part in display.text[piece.display.clone()].split('\n') {
                found.push((line, column..column + part.len(), highlight));
                line += 1;
                column = 0;
            }
        }
        found
    }

    /// The text the widget shows: the model's, or in review mode the text with the suggestions.
    fn shown(&self) -> &str {
        match &self.display {
            Some(display) => &display.text,
            None => self.model.text(),
        }
    }

    /// Makes the widget show the text with the waiting suggestions spliced in, or just the text
    /// when there are none. Only the part that differs is copied over, so the view keeps its
    /// place.
    fn show_review(&mut self) {
        let suggestions = self.review.suggestions();
        self.display =
            (!suggestions.is_empty()).then(|| DisplayMap::new(self.model.text(), suggestions));
        let old = self.content.text();
        if let Some(change) = TextModel::change_between(&old, self.shown(), old.len()) {
            self.copy_change(&old, &change);
        }
        debug_assert_eq!(self.shown(), self.content.text());
    }

    /// Does what the widget asked. An edit is recorded for undo; returns whether the text
    /// changed. Edits are dropped while suggestions wait.
    pub fn perform(&mut self, action: Action) -> bool {
        if !action.is_edit() {
            self.content.perform(action);
            return false;
        }
        if self.reviewing() {
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

    /// The selected text as a byte range of the model's text, or `None` when nothing is
    /// selected. In review mode a selection of only ghost text selects nothing.
    pub fn selection(&self) -> Option<Range<usize>> {
        let cursor = self.content.cursor();
        let anchor = cursor.selection?;
        let (a, b) = (self.to_text(cursor.position), self.to_text(anchor));
        (a != b).then(|| a.min(b)..a.max(b))
    }

    /// The selected text and its paint, for Ctrl+C and Ctrl+X: the model's text, never the
    /// suggestions shown in it.
    pub fn copied(&self) -> Option<(String, Vec<Span>)> {
        self.selection().map(|range| self.model.clipped(range))
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
        if self.reviewing() {
            return false;
        }
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

    /// Copies changes the model made into the widget, in order. This keeps the scroll position
    /// and leaves the caret at the end of the last change. In review mode the suggestions move
    /// with the changes (one a change touches is dropped) and the widget shows the result.
    /// Returns whether there were any changes.
    fn replay(&mut self, changes: &[Change]) -> bool {
        if self.reviewing() {
            for change in changes {
                self.review.shift(change);
            }
            self.show_review();
            return !changes.is_empty();
        }
        let mut text = self.content.text();
        for change in changes {
            self.copy_change(&text, change);
            text.replace_range(
                change.at..change.at + change.removed.len(),
                &change.inserted,
            );
        }
        debug_assert_eq!(self.model.text(), self.content.text());
        !changes.is_empty()
    }

    /// Copies one change into the widget, whose text is `text`: select what was removed, then
    /// paste over it (or delete it).
    fn copy_change(&mut self, text: &str, change: &Change) {
        let (line, column) = position(text, change.at);
        let (end_line, end_column) = position(text, change.at + change.removed.len());
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
    }
}

/// What the highlighter colours: XML tags and macros when `markup` is on, and in review mode the
/// struck and ghost text and the room for the buttons, as (line, byte columns, look).
#[derive(Debug, Clone, PartialEq)]
pub struct Colouring {
    markup: bool,
    review: Vec<(usize, Range<usize>, Highlight)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Highlight {
    Markup(Markup),
    /// Text a suggestion would replace: muted (the line through it is drawn as paint).
    Struck,
    /// A suggestion's new text: italic.
    Ghost,
    /// The room for a suggestion's buttons: invisible, as the buttons are drawn over it.
    Hidden,
}

fn highlight_format(highlight: &Highlight, theme: &Theme) -> highlighter::Format<Font> {
    let roles = Roles::of(theme);
    let color = match highlight {
        Highlight::Markup(Markup::Tag) | Highlight::Struck => Some(roles.fg_muted),
        Highlight::Markup(Markup::Macro) => Some(roles.accent_cool),
        Highlight::Hidden => Some(Color::TRANSPARENT),
        Highlight::Ghost => None,
    };
    let font = (*highlight == Highlight::Ghost).then_some(Font {
        style: iced::font::Style::Italic,
        ..MONO
    });
    highlighter::Format { color, font }
}

/// iced asks for colours one line at a time; `line` tracks how far it has got.
pub struct MarkupHighlighter {
    line: usize,
    colouring: Colouring,
}

impl Highlighter for MarkupHighlighter {
    type Settings = Colouring;
    type Highlight = Highlight;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Highlight)>;

    fn new(colouring: &Colouring) -> Self {
        MarkupHighlighter {
            line: 0,
            colouring: colouring.clone(),
        }
    }

    fn update(&mut self, colouring: &Colouring) {
        self.line = 0;
        self.colouring = colouring.clone();
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    /// Tags and macros first, so suggestion looks win where they overlap.
    fn highlight_line(&mut self, text: &str) -> Self::Iterator<'_> {
        let mut found = Vec::new();
        if self.colouring.markup {
            for (range, markup) in markup_ranges(text) {
                found.push((range, Highlight::Markup(markup)));
            }
        }
        for (line, range, highlight) in &self.colouring.review {
            if *line == self.line {
                found.push((range.clone(), *highlight));
            }
        }
        self.line += 1;
        found.into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::review::{BUTTON_ROOM, GHOST_START};
    use iced::keyboard::Modifiers;
    use iced::keyboard::key::{Code, Physical};

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

    /// A pane showing "Be quick. Stay calm." with "Be quick." to become "Take your time." (in
    /// idea 3) and "calm" to become "steady".
    fn reviewing_pane() -> (Pane, u64, u64) {
        let mut pane = Pane::new("Be quick. Stay calm.");
        let first = pane.suggest(0..9, "Take your time.", Some(3)).unwrap();
        let second = pane.suggest(15..19, "steady", None).unwrap();
        (pane, first, second)
    }

    #[test]
    fn suggestions_show_in_the_widget_but_never_in_the_model() {
        let (mut pane, _, _) = reviewing_pane();
        let (z, room) = (GHOST_START, BUTTON_ROOM);
        assert!(pane.reviewing());
        assert_eq!(
            pane.content.text(),
            format!("Be quick.{z}Take your time.{room} Stay calm{z}steady{room}.")
        );
        assert_eq!(pane.model.text(), "Be quick. Stay calm.");
        // Typing is off; moving still works.
        pane.content.move_to(cursor_at(0, 3));
        assert!(!pane.perform(Action::Edit(Edit::Insert('x'))));
        assert!(!pane.perform(Action::Edit(Edit::Paste(Arc::new("y".to_string())))));
        pane.perform(Action::Move(Motion::Right));
        assert_eq!(pane.content.cursor().position.column, 4);
        assert_eq!(pane.model.text(), "Be quick. Stay calm.");
        assert!(!pane.unindent(4));
    }

    #[test]
    fn accept_is_one_undo_step_and_reject_drops_the_suggestion() {
        let (mut pane, first, second) = reviewing_pane();
        let (z, room) = (GHOST_START, BUTTON_ROOM);
        assert!(pane.decide(Decision::Accept(Some(first))));
        assert_eq!(pane.model.text(), "Take your time. Stay calm.");
        assert_eq!(
            pane.content.text(),
            format!("Take your time. Stay calm{z}steady{room}.")
        );
        // The accepted text took idea 3's paint; the other suggestion moved along.
        assert_eq!((pane.model.paint[0].idea, pane.model.paint[0].end), (3, 15));
        assert_eq!(pane.suggestions()[0].range, 21..25);

        assert!(!pane.decide(Decision::Reject(Some(second))));
        assert!(!pane.reviewing());
        assert_eq!(pane.content.text(), "Take your time. Stay calm.");
        // Typing works again, and one undo takes the accepted text back out.
        pane.content.move_to(cursor_at(0, 0));
        assert!(pane.perform(Action::Edit(Edit::Insert('>'))));
        pane.undo();
        pane.undo();
        assert_eq!(pane.content.text(), "Be quick. Stay calm.");
        assert!(pane.model.paint.is_empty());
    }

    #[test]
    fn tab_and_esc_decide_the_suggestion_at_the_caret() {
        let (mut pane, _, _) = reviewing_pane();
        // The caret in " Stay": the next suggestion is "calm" -> "steady".
        let stay = pane.position(11, false);
        pane.content.move_to(Cursor {
            position: stay,
            selection: None,
        });
        assert!(pane.decide(Decision::Accept(None)));
        assert_eq!(pane.model.text(), "Be quick. Stay steady.");
        assert!(!pane.decide(Decision::Reject(None)), "the last one left");
        assert_eq!(pane.content.text(), "Be quick. Stay steady.");
    }

    #[test]
    fn suggesting_and_rejecting_leave_the_caret_where_it_was() {
        let mut pane = Pane::new("Be quick. Stay calm.");
        pane.content.move_to(cursor_at(0, 12));
        let id = pane.suggest(15..19, "steady", None).unwrap();
        pane.suggest(0..9, "Take your time.", None).unwrap();
        assert_eq!(pane.content.cursor().position, pane.position(12, true));
        assert!(!pane.decide(Decision::Reject(Some(id))));
        assert_eq!(pane.content.cursor().position, pane.position(12, true));
        // Accepting puts the caret after the new text.
        assert!(pane.decide(Decision::AcceptAll));
        assert_eq!(pane.content.cursor().position, cursor_at(0, 15).position);
    }

    #[test]
    fn accept_all_and_reject_all() {
        let (mut pane, _, _) = reviewing_pane();
        assert!(pane.decide(Decision::AcceptAll));
        assert_eq!(pane.content.text(), "Take your time. Stay steady.");
        assert!(pane.undo(), "both were one step");
        assert_eq!(pane.content.text(), "Be quick. Stay calm.");

        let (mut pane, _, _) = reviewing_pane();
        assert!(!pane.decide(Decision::RejectAll));
        assert_eq!(pane.content.text(), "Be quick. Stay calm.");
        assert!(!pane.reviewing());
    }

    #[test]
    fn selection_and_positions_map_between_the_text_and_the_widget() {
        let (mut pane, _, _) = reviewing_pane();
        let shown_offset =
            |position: Position| offset(&pane.content.text(), position.line, position.column);
        // " Stay " starts after the first suggestion's ghost text and buttons.
        let stay = pane.position(9, true);
        assert_eq!(&pane.content.text()[shown_offset(stay)..][..5], " Stay");
        assert_eq!(shown_offset(pane.position(9, false)), 9);
        // Selecting from inside "quick" to inside "Stay" selects that text of the model.
        pane.content.move_to(Cursor {
            position: pane.position(12, true),
            selection: Some(pane.position(4, true)),
        });
        assert_eq!(pane.selection(), Some(4..12));
        // A selection of only ghost text is no selection.
        pane.content.move_to(Cursor {
            position: Position {
                line: 0,
                column: 17,
            },
            selection: Some(Position {
                line: 0,
                column: 12,
            }),
        });
        assert_eq!(pane.selection(), None);
    }

    #[test]
    fn copying_while_reviewing_leaves_the_suggestions_out() {
        let ctrl_c = KeyPress {
            key: Key::Character("c".into()),
            modified_key: Key::Character("c".into()),
            physical_key: Physical::Code(Code::KeyC),
            modifiers: Modifiers::CTRL,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        let binding = |reviewing| key_binding(ctrl_c.clone(), 4, true, reviewing, EDITOR_ID);
        assert!(matches!(binding(false), Some(Binding::Sequence(_))));
        assert!(matches!(
            binding(true),
            Some(Binding::Custom(Message::Clip(EDITOR_ID)))
        ));
        // `Clip` copies the model's text under the selection, without ghost text or buttons.
        let (mut pane, _, _) = reviewing_pane();
        pane.content.move_to(Cursor {
            position: pane.position(12, true),
            selection: Some(pane.position(4, true)),
        });
        assert_eq!(
            pane.copied().map(|(text, _)| text).as_deref(),
            Some("uick. St")
        );
    }

    #[test]
    fn undo_and_replace_while_reviewing_move_or_drop_suggestions() {
        let mut pane = Pane::new("one two");
        pane.replace(0..3, "ONE");
        let id = pane.suggest(4..7, "2", None).unwrap();
        // Undoing the edit before the suggestion moves it with the text.
        assert!(pane.undo());
        assert_eq!(pane.model.text(), "one two");
        assert_eq!(pane.suggestions()[0].range, 4..7);
        // Replacing the whole text (a reload from disk) drops it.
        pane.replace(0..7, "fresh");
        assert!(pane.suggestions().iter().all(|s| s.id != id));
        assert!(!pane.reviewing());
        assert_eq!(pane.content.text(), "fresh");
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
