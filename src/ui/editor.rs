//! The text editing widget: the text column, Tab as spaces, Enter and Backspace that know about
//! indents, deleting a word, undo and redo (its colours come from `highlighter`). A `Pane` pairs
//! the widget's content with the text model behind it, and shows suggested edits inside the text
//! until each is accepted or rejected (review mode).

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use iced::keyboard::{Key, key::Named};
use iced::widget::container;
use iced::widget::text::Wrapping;
use iced::widget::text_editor::Motion;
use iced::{Background, Border, Element, Fill, Padding, Theme};

use crate::core::review::{DisplayMap, Review, Shown, Suggestion, clashes};
use crate::core::settings::{ReadingMode, Settings};
use crate::core::text_file::newlines_only;
use crate::core::text_model::{Change, TextModel, offset, position, positions};
use crate::ui::app::Message;
use crate::ui::highlighter::{Colouring, Highlight, MarkupHighlighter, highlight_format};
use crate::ui::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, PaintLook, PaintRun, Position, Status,
    TextEditor,
};
use crate::ui::theme::{CHAR_WIDTH_RATIO, Roles};

pub const EDITOR_ID: &str = "doc";
/// Why an edit to the text a suggestion changes was refused.
const IN_A_SUGGESTION: &str = "Accept or reject the suggestion there first";
pub const LINE_HEIGHT: f32 = 1.5;
/// Text starts near the top left, like any text editor, with just enough room to breathe.
pub const PADDING: Padding = Padding {
    top: 12.0,
    right: 16.0,
    bottom: 24.0,
    left: 16.0,
};

/// The pane's editor in its column, with `paint` (any suggestions, from `Pane::suggestion_runs`,
/// and find matches) drawn under the text, and Markdown coloured when `markdown` is set. Without
/// `editable` the text is dimmed; it can still be scrolled, selected and copied, and
/// `App::update` drops any change to it.
pub fn view<'a>(
    pane: &'a Pane,
    settings: &Settings,
    editable: bool,
    markdown: bool,
    paint: Vec<PaintRun>,
) -> Element<'a, Message> {
    let reviewing = pane.reviewing();
    let colouring = Colouring {
        markdown,
        markup: settings.colour_markup,
        review: pane.review_highlights(),
    };
    let editor = TextEditor::new(&pane.content)
        .id(EDITOR_ID)
        .key_binding(move |press| key_binding(press, editable, reviewing))
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
        .on_action(Message::Edit)
        .paint(paint)
        .on_decide(|id, accept| {
            Message::Decide(if accept {
                Decision::Accept(Some(id))
            } else {
                Decision::Reject(Some(id))
            })
        })
        .highlight_with::<MarkupHighlighter>(colouring, highlight_format);

    let column = match settings.reading_mode {
        ReadingMode::Comfort { chars } => {
            let text_width = f32::from(chars) * settings.font_size * CHAR_WIDTH_RATIO;
            container(editor).max_width(text_width + PADDING.left + PADDING.right)
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

/// Tab indents (with spaces, never a tab character), Shift+Tab unindents, Enter (and Shift+Enter)
/// starts a line indented like the caret's, Backspace in a line's leading spaces goes back to the
/// last indent stop, Ctrl+Backspace and Ctrl+Delete delete a word (⌥⌫ and ⌥⌦ on macOS, where ⌘⌫
/// and ⌘⌦ delete to the start and end of the line), Ctrl+Z undoes, Ctrl+Shift+Z and Ctrl+Y redo,
/// and the app's own Ctrl shortcuts are left for the app. A pane that isn't `editable` keeps only
/// iced's own keys (moving, selecting, copying). While `reviewing`, Tab accepts the suggestion the
/// caret is on and Esc rejects it (`App::update` turns a Tab anywhere else back into an indent),
/// and Ctrl+C and Ctrl+X go through `Message::Clip`, since the widget's text has the suggestions
/// in it.
fn key_binding(press: KeyPress, editable: bool, reviewing: bool) -> Option<Binding<Message>> {
    // A custom binding is used even when the editor isn't focused, so check first.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if press.modifiers.command()
        && matches!(
            press.key.as_ref(),
            Key::Character(
                "p" | "0" | "," | "l" | "w" | "W" | "s" | "S" | "o" | "n" | "f" | "=" | "+" | "-"
            )
        )
    {
        return None;
    }
    // macOS: ⌘↑ and ⌘↓ go to the start and end of the text, as ⌘← and ⌘→ go to the line's.
    if press.modifiers.macos_command()
        && let Key::Named(arrow @ (Named::ArrowUp | Named::ArrowDown)) = press.key.as_ref()
    {
        let motion = match arrow {
            Named::ArrowUp => Motion::DocumentStart,
            _ => Motion::DocumentEnd,
        };
        return Some(match press.modifiers.shift() {
            true => Binding::Select(motion),
            false => Binding::Move(motion),
        });
    }
    if reviewing
        && let Some(clip @ (Binding::<Message>::Cut | Binding::Copy)) =
            Binding::from_key_press(press.clone())
    {
        let cut = matches!(clip, Binding::Cut);
        return Some(Binding::Custom(Message::Clip { cut }));
    }
    if !editable {
        return Binding::from_key_press(press);
    }
    if let Key::Named(key @ (Named::Backspace | Named::Delete)) = press.key.as_ref()
        && (press.modifiers.jump() || press.modifiers.macos_command())
    {
        let motion = match (key, press.modifiers.macos_command()) {
            (Named::Backspace, true) => Motion::Home,
            (Named::Backspace, false) => Motion::WordLeft,
            (_, true) => Motion::End,
            (_, false) => Motion::WordRight,
        };
        return Some(Binding::Custom(Message::DeleteTo(motion)));
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
        Key::Named(Named::Tab) => Some(Binding::Custom(Message::Indent)),
        Key::Named(Named::Enter) => Some(Binding::Custom(Message::NewLine)),
        Key::Named(Named::Backspace) => Some(Binding::Custom(Message::Backspace)),
        _ => Binding::from_key_press(press),
    }
}

/// What to do with suggested edits: accept or reject one (by id; `None` is the one the caret is
/// on, if any), or all of them.
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
    /// how it maps back to the text.
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

    /// Whether suggestions are waiting (review mode). The text around them can still be edited,
    /// but not the text they change (see `perform`).
    pub fn reviewing(&self) -> bool {
        self.display.is_some()
    }

    /// The suggestions waiting, in text order.
    pub fn suggestions(&self) -> &[Suggestion] {
        self.review.suggestions()
    }

    /// Shows a suggestion to put `text` in place of `range` (byte offsets into the model's
    /// text). The caret stays where it is. Returns its id, or why it was refused.
    pub fn suggest(&mut self, range: Range<usize>, text: &str) -> Result<u64, String> {
        let caret = self.to_text(self.content.cursor().position);
        let id = self
            .review
            .add(self.model.text(), range, &newlines_only(text))?;
        self.show_review();
        self.put_caret(caret);
        Ok(id)
    }

    /// Accepts or rejects suggestions. Accepting puts the new text in as one undo step and leaves
    /// the caret after it; rejecting leaves the caret where it was. Returns whether the text
    /// changed.
    pub fn decide(&mut self, decision: Decision) -> bool {
        let at_caret = self.suggestion_at_caret();
        let caret = self.caret();
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

    /// The suggestion the caret is on (in or touching its struck text, ghost text or buttons),
    /// if any.
    pub fn suggestion_at_caret(&self) -> Option<u64> {
        let caret = self.content.cursor().position;
        let display = self.display.as_ref()?;
        display.suggestion_at(offset(&display.text, caret.line, caret.column))
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

    /// Where each of `ranges` (byte ranges of the model's text, in order) is in the widget,
    /// worked out in one pass over the text, for drawing many at once (the find matches).
    pub fn widget_ranges(&self, ranges: &[Range<usize>]) -> Vec<(Position, Position)> {
        let shown = |offset| match &self.display {
            Some(display) => display.to_display(offset, false),
            None => offset,
        };
        let offsets: Vec<usize> = ranges
            .iter()
            .flat_map(|range| [shown(range.start), shown(range.end)])
            .collect();
        let at = |(line, column)| Position { line, column };
        positions(self.shown(), &offsets)
            .chunks(2)
            .map(|pair| (at(pair[0]), at(pair[1])))
            .collect()
    }

    /// Selects byte range `range` of the model's text with the caret at its end, which scrolls
    /// it into view.
    pub fn select(&mut self, range: Range<usize>) {
        let start = self.position(range.start, false);
        let end = self.position(range.end, false);
        self.content.move_to(Cursor {
            position: end,
            selection: Some(start),
        });
    }

    /// The suggestions as the editor draws them: the replaced text struck through, the new text
    /// as ghost text, and room for the buttons, in the cool accent.
    pub fn suggestion_runs(&self, roles: &Roles) -> Vec<PaintRun> {
        let Some(display) = &self.display else {
            return Vec::new();
        };
        let at = |offset| {
            let (line, column) = position(&display.text, offset);
            Position { line, column }
        };
        let mut runs = Vec::new();
        for piece in display.pieces.iter().filter(|p| !p.display.is_empty()) {
            let Some(id) = piece.suggestion else {
                continue;
            };
            let look = match piece.shown {
                Shown::Text => continue,
                Shown::Struck => PaintLook::Struck(roles.fg_subtle),
                Shown::Ghost => PaintLook::Ghost,
                Shown::Buttons => PaintLook::Buttons { id },
            };
            runs.push(PaintRun {
                start: at(piece.display.start),
                end: at(piece.display.end),
                color: roles.accent_cool,
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
    /// changed. Waiting suggestions move with an edit to the text around them, and an edit to
    /// the text they change (or to their ghost text) is refused, with the reason.
    pub fn perform(&mut self, action: Action) -> Result<bool, String> {
        if !action.is_edit() {
            self.content.perform(action);
            return Ok(false);
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
        let text = self.shown();
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
        let Some(display) = &self.display else {
            let changed = self
                .model
                .record(self.content.text(), edit_start, Instant::now())
                .is_some();
            debug_assert_eq!(self.model.text(), self.content.text());
            return Ok(changed);
        };
        // The widget's text has the suggestions in it. The edit counts only if it is all in one
        // piece of plain text, which is the same in the model's text.
        let Some(change) =
            TextModel::change_between(&display.text, &self.content.text(), edit_start)
        else {
            return Ok(false);
        };
        let removed_end = change.at + change.removed.len();
        let plain = display.pieces.iter().find(|piece| {
            piece.shown == Shown::Text
                && piece.display.start <= change.at
                && removed_end <= piece.display.end
        });
        let Some(piece) = plain else {
            // Put the widget's text and caret back as they were.
            self.show_review();
            self.content.move_to(cursor);
            return Err(IN_A_SUGGESTION.to_string());
        };
        let at = piece.text.start + change.at - piece.display.start;
        // Typed right before a pure insert's ghost text (`at` is where the insert sits too), the
        // insert moves along so the typing lands at the caret. Not at the end of the display,
        // which is after the last suggestion's buttons.
        let ahead = change.at == piece.display.end && change.at < display.text.len();
        let mut text = self.model.text().to_string();
        text.replace_range(at..at + change.removed.len(), &change.inserted);
        let recorded = self.model.record(text, at, Instant::now());
        if let Some(change) = &recorded {
            self.review.shift(change, ahead);
        }
        self.show_review();
        Ok(recorded.is_some())
    }

    /// The selected text as a byte range of the model's text, or `None` when nothing is
    /// selected. In review mode a selection of only ghost text selects nothing.
    pub fn selection(&self) -> Option<Range<usize>> {
        let cursor = self.content.cursor();
        let anchor = cursor.selection?;
        let (a, b) = (self.to_text(cursor.position), self.to_text(anchor));
        (a != b).then(|| a.min(b)..a.max(b))
    }

    /// The selected text, for Ctrl+C and Ctrl+X while reviewing: the model's text, never the
    /// suggestions shown in it.
    pub fn copied(&self) -> Option<String> {
        self.selection()
            .map(|range| self.model.text()[range].to_string())
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

    /// Replaces `range` (byte offsets) with `text` as one undo step, such as a reload from disk.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let change = self.model.replace(range, text);
        self.replay(&[change]);
    }

    /// Tab: adds one indent (`tab_spaces` spaces) to the start of each line the selection is on.
    /// With nothing selected, or a selection within one line, it types the spaces instead.
    /// Returns whether the text changed, or why it can't.
    pub fn indent(&mut self, tab_spaces: usize) -> Result<bool, String> {
        self.reindent(tab_spaces, true)
    }

    /// Shift+Tab: removes up to one indent (`tab_spaces` spaces) from the start of the caret's
    /// line, or of each line the selection is on. Returns whether the text changed, or why it
    /// can't.
    pub fn unindent(&mut self, tab_spaces: usize) -> Result<bool, String> {
        self.reindent(tab_spaces, false)
    }

    /// Enter (and Shift+Enter): a new line in place of any selection, indented like the caret's
    /// line up to the caret. When only spaces are before the caret they move down with it, so the
    /// line left behind is empty rather than trailing spaces. Returns whether the text changed,
    /// or why it can't.
    pub fn new_line(&mut self) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let caret = self.caret();
        let range = self.selection().unwrap_or(caret..caret);
        let text = self.model.text();
        let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
        let before = &text[line_start..range.start];
        let indent = &before[..before.len() - before.trim_start_matches([' ', '\t']).len()];
        let typed = Edit::Paste(Arc::new(format!("\n{indent}")));
        if indent.is_empty() || indent.len() < before.len() {
            return self.perform(Action::Edit(typed));
        }
        self.content.move_to(Cursor {
            position: self.position(range.end, false),
            selection: Some(self.position(line_start, true)),
        });
        self.perform_or_put_back(typed, cursor)
    }

    /// Backspace. In the spaces at the start of a line it deletes back to the previous indent
    /// stop (a multiple of `tab_spaces`), so one press takes back a Tab; anywhere else, or with a
    /// selection, it deletes as usual. Returns whether the text changed, or why it can't.
    pub fn backspace(&mut self, tab_spaces: usize) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let caret = self.caret();
        let text = self.model.text();
        let line_start = text[..caret].rfind('\n').map_or(0, |at| at + 1);
        let column = caret - line_start;
        let in_indent = column > 0 && text[line_start..caret].bytes().all(|byte| byte == b' ');
        if self.selection().is_some() || !in_indent {
            return self.perform(Action::Edit(Edit::Backspace));
        }
        let stop = (column - 1) / tab_spaces * tab_spaces;
        self.content.move_to(Cursor {
            position: cursor.position,
            selection: Some(self.position(line_start + stop, true)),
        });
        self.perform_or_put_back(Edit::Delete, cursor)
    }

    /// Ctrl+Backspace and Ctrl+Delete, and ⌘⌫ and ⌘⌦ on macOS: deletes from the caret to where
    /// `motion` takes it (the start of the word before, the end of the word after, or the start
    /// or end of the line), or just the selection when there is one. The widget's own motion is
    /// used, so a word is what Ctrl+← and Ctrl+→ step over. Returns whether the text changed, or
    /// why it can't.
    pub fn delete_to(&mut self, motion: Motion) -> Result<bool, String> {
        let cursor = self.content.cursor();
        if cursor.selection.is_none() {
            self.content.perform(Action::Select(motion));
            if self.content.cursor().selection.is_none() {
                return Ok(false);
            }
        }
        self.perform_or_put_back(Edit::Delete, cursor)
    }

    /// Does `edit` where the caret and selection have just been put, as `perform` does any edit.
    /// If it's refused, they go back to `before`.
    fn perform_or_put_back(&mut self, edit: Edit, before: Cursor) -> Result<bool, String> {
        let edited = self.perform(Action::Edit(edit));
        if edited.is_err() {
            self.content.move_to(before);
        }
        edited
    }

    /// Indents (`add`) or unindents the lines for `indent` and `unindent`, as one undo step, and
    /// keeps the caret and selection on the same text. A last line the selection only reaches
    /// the start of is left alone, as nothing on it is selected, and lines with a suggestion on
    /// them are refused.
    fn reindent(&mut self, tab_spaces: usize, add: bool) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let caret = self.to_text(cursor.position);
        let anchor = cursor.selection.map_or(caret, |at| self.to_text(at));
        let (start, end) = (caret.min(anchor), caret.max(anchor));
        let text = self.model.text();
        if add && !text[start..end].contains('\n') {
            let spaces = Arc::new(" ".repeat(tab_spaces));
            return self.perform(Action::Edit(Edit::Paste(spaces)));
        }
        let (first, _) = position(text, start);
        let (mut last, column) = position(text, end);
        if column == 0 && last > first {
            last -= 1;
        }
        let lines = offset(text, first, 0)..offset(text, last + 1, 0);
        if self.suggestions().iter().any(|s| clashes(&s.range, &lines)) {
            return Err(IN_A_SUGGESTION.to_string());
        }
        let changes = match add {
            true => self.model.indent(first..=last, tab_spaces),
            false => self.model.unindent(first..=last, tab_spaces),
        };
        if !self.replay(&changes) {
            return Ok(false);
        }
        // Each change is at the start of a line: what is after it moves with it, and the start
        // of the line stays put.
        let moved = |mut at: usize| {
            for change in &changes {
                if at > change.at {
                    let removed_end = change.at + change.removed.len();
                    at = at.max(removed_end) - change.removed.len() + change.inserted.len();
                }
            }
            at
        };
        let position = self.position(moved(caret), true);
        let selection = cursor.selection.map(|_| self.position(moved(anchor), true));
        self.content.move_to(Cursor {
            position,
            selection,
        });
        Ok(true)
    }

    /// Copies changes the model made into the widget, in order. This keeps the scroll position
    /// and leaves the caret at the end of the last change. In review mode the suggestions move
    /// with the changes (one a change touches is dropped) and the widget shows the result.
    /// Returns whether there were any changes.
    fn replay(&mut self, changes: &[Change]) -> bool {
        if self.reviewing() {
            for change in changes {
                self.review.shift(change, false);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::review::{BUTTON_ROOM, GHOST_START};
    use iced::keyboard::Modifiers;
    use iced::keyboard::key::{Code, NativeCode, Physical};

    fn cursor_at(line: usize, column: usize) -> Cursor {
        Cursor {
            position: Position { line, column },
            selection: None,
        }
    }

    /// What `key_binding` makes of `key` with `modifiers`, in an editable pane not reviewing.
    fn binding_for(key: Named, modifiers: Modifiers) -> Option<Binding<Message>> {
        let press = KeyPress {
            key: Key::Named(key),
            modified_key: Key::Named(key),
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        key_binding(press, true, false)
    }

    #[test]
    fn edits_are_recorded_and_undo_redo_replay_into_the_widget() {
        let mut pane = Pane::new("hello world");
        pane.content.move_to(cursor_at(0, 5));
        for c in ",there".chars() {
            assert_eq!(pane.perform(Action::Edit(Edit::Insert(c))), Ok(true));
        }
        pane.perform(Action::Edit(Edit::Backspace)).unwrap();
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
        pane.perform(Action::Edit(Edit::Paste(Arc::new("X\nY".to_string()))))
            .unwrap();
        assert_eq!(pane.content.text(), "oX\nYree");
        pane.undo();
        assert_eq!(pane.content.text(), "one\ntwo\nthree");
    }

    #[test]
    fn pasted_carriage_returns_keep_the_widget_and_model_in_step() {
        let mut pane = Pane::new("");
        pane.perform(Action::Edit(Edit::Paste(Arc::new(
            "a\rb\n\réé".to_string(),
        ))))
        .unwrap();
        assert_eq!(pane.content.text(), "a\nb\n\néé");
        pane.content.move_to(cursor_at(1, 1));
        assert_eq!(pane.perform(Action::Edit(Edit::Backspace)), Ok(true));
        assert_eq!(pane.content.text(), "a\n\n\néé");
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "a\nb\n\néé");
        pane.replace(0..pane.model.text().len(), "x\ny");
        assert_eq!(pane.content.text(), "x\ny");
    }

    #[test]
    fn selection_is_a_byte_range() {
        let mut pane = Pane::new("café au lait");
        pane.content.move_to(Cursor {
            position: Position { line: 0, column: 2 },
            selection: Some(Position { line: 0, column: 9 }),
        });
        assert_eq!(pane.selection(), Some(2..9));
        // Selected the other way, it's the same range.
        pane.content.move_to(Cursor {
            position: Position { line: 0, column: 9 },
            selection: Some(Position { line: 0, column: 2 }),
        });
        assert_eq!(pane.selection(), Some(2..9));
        assert_eq!(Pane::new("café").selection(), None);
    }

    /// A pane showing "Be quick. Stay calm." with "Be quick." to become "Take your time." and
    /// "calm" to become "steady".
    fn reviewing_pane() -> (Pane, u64, u64) {
        let mut pane = Pane::new("Be quick. Stay calm.");
        let first = pane.suggest(0..9, "Take your time.").unwrap();
        let second = pane.suggest(15..19, "steady").unwrap();
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
        // Typing into the text a suggestion changes is refused and leaves everything as it was;
        // moving still works.
        pane.content.move_to(cursor_at(0, 3));
        let shown = pane.content.text();
        let refused = Err(IN_A_SUGGESTION.to_string());
        assert_eq!(pane.perform(Action::Edit(Edit::Insert('x'))), refused);
        let pasted = Edit::Paste(Arc::new("y".to_string()));
        assert_eq!(pane.perform(Action::Edit(pasted)), refused);
        assert_eq!(pane.content.text(), shown);
        assert_eq!(pane.content.cursor(), cursor_at(0, 3));
        pane.perform(Action::Move(Motion::Right)).unwrap();
        assert_eq!(pane.content.cursor().position.column, 4);
        assert_eq!(pane.model.text(), "Be quick. Stay calm.");
        assert_eq!(pane.unindent(4), refused, "the line has a suggestion on it");
    }

    #[test]
    fn typing_around_suggestions_moves_them() {
        let (mut pane, _, _) = reviewing_pane();
        let (z, room) = (GHOST_START, BUTTON_ROOM);
        // Right after the first suggestion's buttons.
        pane.put_caret(9);
        for c in " Now".chars() {
            assert_eq!(pane.perform(Action::Edit(Edit::Insert(c))), Ok(true));
        }
        assert_eq!(pane.model.text(), "Be quick. Now Stay calm.");
        assert_eq!(
            pane.content.text(),
            format!("Be quick.{z}Take your time.{room} Now Stay calm{z}steady{room}.")
        );
        assert_eq!(pane.suggestions()[1].range, 19..23);
        // Backspace there takes the typing back off, but not the "." the suggestion replaces.
        for _ in 0..4 {
            assert_eq!(pane.perform(Action::Edit(Edit::Backspace)), Ok(true));
        }
        assert_eq!(
            pane.perform(Action::Edit(Edit::Backspace)),
            Err(IN_A_SUGGESTION.to_string())
        );
        assert_eq!(pane.model.text(), "Be quick. Stay calm.");
        // Enter before the first suggestion pushes both along; one undo takes it back out.
        pane.put_caret(0);
        assert_eq!(pane.perform(Action::Edit(Edit::Enter)), Ok(true));
        assert_eq!(pane.suggestions()[0].range, 1..10);
        assert!(pane.undo());
        assert_eq!(pane.suggestions()[0].range, 0..9);
        assert_eq!(
            pane.content.text(),
            format!("Be quick.{z}Take your time.{room} Stay calm{z}steady{room}.")
        );
        // Pasting over a selection that reaches into a suggestion is refused.
        pane.select(11..17);
        let pasted = Edit::Paste(Arc::new("x".to_string()));
        assert_eq!(
            pane.perform(Action::Edit(pasted)),
            Err(IN_A_SUGGESTION.to_string())
        );
        assert_eq!(pane.model.text(), "Be quick. Stay calm.");
    }

    #[test]
    fn typing_right_before_an_insert_goes_before_its_ghost_text() {
        let (z, room) = (GHOST_START, BUTTON_ROOM);
        let mut pane = Pane::new("ab cd");
        pane.suggest(2..2, "XY").unwrap();
        pane.content.move_to(cursor_at(0, 2));
        assert_eq!(pane.perform(Action::Edit(Edit::Insert('!'))), Ok(true));
        assert_eq!(pane.content.text(), format!("ab!{z}XY{room} cd"));
        assert_eq!(pane.content.cursor(), cursor_at(0, 3));
        assert!(pane.decide(Decision::AcceptAll));
        assert_eq!(pane.model.text(), "ab!XY cd");
        // After an insert's buttons at the end of the text, typing still goes after it.
        let mut pane = Pane::new("Hi.");
        pane.suggest(3..3, " Bye.").unwrap();
        pane.put_caret(3);
        assert_eq!(pane.perform(Action::Edit(Edit::Insert('!'))), Ok(true));
        assert_eq!(pane.content.text(), format!("Hi.{z} Bye.{room}!"));
        assert_eq!(pane.suggestions()[0].range, 3..3);
    }

    #[test]
    fn shift_tab_while_reviewing_leaves_lines_with_suggestions() {
        let mut pane = Pane::new("    one\n    two");
        pane.suggest(12..15, "2").unwrap();
        pane.put_caret(5);
        assert_eq!(pane.unindent(4), Ok(true));
        assert_eq!(pane.model.text(), "one\n    two");
        assert_eq!(pane.suggestions()[0].range, 8..11);
        assert_eq!(pane.caret(), 1, "the caret stays after the \"o\"");
        pane.put_caret(10);
        assert_eq!(pane.unindent(4), Err(IN_A_SUGGESTION.to_string()));
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
        // The other suggestion moved along.
        assert_eq!(pane.suggestions()[0].range, 21..25);

        assert!(!pane.decide(Decision::Reject(Some(second))));
        assert!(!pane.reviewing());
        assert_eq!(pane.content.text(), "Take your time. Stay calm.");
        // Typing works again, and one undo takes the accepted text back out.
        pane.content.move_to(cursor_at(0, 0));
        assert_eq!(pane.perform(Action::Edit(Edit::Insert('>'))), Ok(true));
        pane.undo();
        pane.undo();
        assert_eq!(pane.content.text(), "Be quick. Stay calm.");
    }

    #[test]
    fn tab_and_esc_decide_only_the_suggestion_the_caret_is_on() {
        let (mut pane, first, second) = reviewing_pane();
        // In " Stay", away from both suggestions, Tab and Esc decide nothing (Tab indents).
        pane.put_caret(11);
        assert_eq!(pane.suggestion_at_caret(), None);
        assert!(!pane.decide(Decision::Accept(None)));
        assert!(!pane.decide(Decision::Reject(None)));
        assert_eq!(pane.suggestions().len(), 2);
        // On "calm", Tab accepts that one.
        pane.put_caret(16);
        assert_eq!(pane.suggestion_at_caret(), Some(second));
        assert!(pane.decide(Decision::Accept(None)));
        assert_eq!(pane.model.text(), "Be quick. Stay steady.");
        // On "quick", Esc rejects the other.
        pane.put_caret(4);
        assert_eq!(pane.suggestion_at_caret(), Some(first));
        assert!(!pane.decide(Decision::Reject(None)));
        assert!(!pane.reviewing());
        assert_eq!(pane.content.text(), "Be quick. Stay steady.");
    }

    #[test]
    fn suggesting_and_rejecting_leave_the_caret_where_it_was() {
        let mut pane = Pane::new("Be quick. Stay calm.");
        pane.content.move_to(cursor_at(0, 12));
        let id = pane.suggest(15..19, "steady").unwrap();
        pane.suggest(0..9, "Take your time.").unwrap();
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
            modifiers: Modifiers::COMMAND,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        let binding = |reviewing| key_binding(ctrl_c.clone(), true, reviewing);
        assert!(matches!(binding(false), Some(Binding::Copy)));
        assert!(matches!(
            binding(true),
            Some(Binding::Custom(Message::Clip { cut: false }))
        ));
        // `Clip` copies the model's text under the selection, without ghost text or buttons.
        let (mut pane, _, _) = reviewing_pane();
        pane.content.move_to(Cursor {
            position: pane.position(12, true),
            selection: Some(pane.position(4, true)),
        });
        assert_eq!(pane.copied().as_deref(), Some("uick. St"));
    }

    #[test]
    fn command_up_goes_to_the_start_of_the_text_on_macos() {
        let command_up = KeyPress {
            key: Key::Named(Named::ArrowUp),
            modified_key: Key::Named(Named::ArrowUp),
            physical_key: Physical::Code(Code::ArrowUp),
            modifiers: Modifiers::LOGO,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        let Some(Binding::Move(motion)) = key_binding(command_up, true, false) else {
            panic!("⌘↑ doesn't move the caret");
        };
        let start = if cfg!(target_os = "macos") {
            Motion::DocumentStart
        } else {
            Motion::Up
        };
        assert_eq!(motion, start);
    }

    #[test]
    fn undo_and_replace_while_reviewing_move_or_drop_suggestions() {
        let mut pane = Pane::new("one two");
        pane.replace(0..3, "ONE");
        let id = pane.suggest(4..7, "2").unwrap();
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
        assert_eq!(pane.unindent(4), Ok(true));
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
        assert_eq!(pane.unindent(4), Ok(true));
        assert_eq!(pane.content.text(), "two levels\nx");
        assert_eq!(
            pane.content.cursor().position,
            Position { line: 1, column: 1 }
        );

        // One undo puts both lines back.
        pane.undo();
        assert_eq!(pane.content.text(), "    two levels\n  x");
    }

    /// Selects from the start of the text to the start of line 2, as Ctrl+Home then Shift+Down
    /// twice do: only lines 0 and 1 show as selected.
    fn select_two_lines(pane: &mut Pane) {
        pane.content.move_to(Cursor {
            position: Position { line: 2, column: 0 },
            selection: Some(Position { line: 0, column: 0 }),
        });
    }

    #[test]
    fn tab_indents_each_selected_line_as_one_step() {
        let mut pane = Pane::new("alpha\nbeta\ngamma\n");
        select_two_lines(&mut pane);
        assert_eq!(pane.indent(4), Ok(true));
        assert_eq!(pane.content.text(), "    alpha\n    beta\ngamma\n");
        assert_eq!(pane.model.text(), pane.content.text());
        // The same lines stay selected, indents included.
        assert_eq!(pane.selection(), Some(0..19));
        assert_eq!(pane.indent(4), Ok(true), "again, a level deeper");
        assert_eq!(pane.content.text(), "        alpha\n        beta\ngamma\n");
        pane.undo();
        assert_eq!(pane.content.text(), "    alpha\n    beta\ngamma\n");

        // A selection that starts and ends inside lines keeps its ends on the same letters.
        pane.content.move_to(Cursor {
            position: Position { line: 1, column: 6 },
            selection: Some(Position { line: 0, column: 5 }),
        });
        assert_eq!(pane.indent(2), Ok(true));
        assert_eq!(pane.content.text(), "      alpha\n      beta\ngamma\n");
        assert_eq!(pane.copied().as_deref(), Some("lpha\n      be"));
    }

    #[test]
    fn tab_within_one_line_types_spaces() {
        let mut pane = Pane::new("one two");
        pane.content.move_to(cursor_at(0, 3));
        assert_eq!(pane.indent(4), Ok(true));
        assert_eq!(pane.content.text(), "one     two");
        // A selection within one line is replaced, as typing would.
        pane.content.move_to(Cursor {
            position: Position { line: 0, column: 3 },
            selection: Some(Position { line: 0, column: 0 }),
        });
        assert_eq!(pane.indent(2), Ok(true));
        assert_eq!(pane.content.text(), "       two");
        pane.undo();
        assert_eq!(pane.content.text(), "one     two", "one undo step");
    }

    #[test]
    fn shift_tab_leaves_a_line_the_selection_only_reaches_the_start_of() {
        let mut pane = Pane::new("    alpha\n    beta\n    gamma\n");
        select_two_lines(&mut pane);
        assert_eq!(pane.unindent(4), Ok(true));
        assert_eq!(pane.content.text(), "alpha\nbeta\n    gamma\n");
    }

    #[test]
    fn enter_keeps_the_indent_before_the_caret() {
        let mut pane = Pane::new("    one two");
        pane.content.move_to(cursor_at(0, 7));
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "    one\n     two");
        assert_eq!(pane.content.cursor(), cursor_at(1, 4));
        // At the start of an indented line, none of the indent is before the caret.
        pane.content.move_to(cursor_at(1, 0));
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "    one\n\n     two");
        // Tabs and multi-byte text: the indent is copied as it is.
        let mut pane = Pane::new("\t  café");
        pane.content.move_to(cursor_at(0, 8));
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "\t  café\n\t  ");
        assert_eq!(pane.content.cursor(), cursor_at(1, 3));
        let mut pane = Pane::new("");
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "\n");
    }

    #[test]
    fn enter_and_shift_enter_start_an_indented_line() {
        for modifiers in [Modifiers::empty(), Modifiers::SHIFT] {
            assert!(matches!(
                binding_for(Named::Enter, modifiers),
                Some(Binding::Custom(Message::NewLine))
            ));
        }
    }

    #[test]
    fn enter_twice_leaves_an_empty_line_not_trailing_spaces() {
        let mut pane = Pane::new("    item");
        pane.content.move_to(cursor_at(0, 8));
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "    item\n\n    ");
        assert_eq!(pane.content.cursor(), cursor_at(2, 4));
        assert_eq!(pane.model.text(), pane.content.text());
        // Each Enter is one undo step.
        pane.undo();
        assert_eq!(pane.content.text(), "    item\n    ");
        pane.undo();
        assert_eq!(pane.content.text(), "    item");
        // In the middle of an indent, the spaces before the caret go down with it.
        let mut pane = Pane::new("        x");
        pane.content.move_to(cursor_at(0, 4));
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "\n        x");
        assert_eq!(pane.content.cursor(), cursor_at(1, 4));
    }

    #[test]
    fn enter_replaces_a_selection() {
        let mut pane = Pane::new("  one two\nthree");
        pane.content.move_to(Cursor {
            position: Position { line: 1, column: 2 },
            selection: Some(Position { line: 0, column: 6 }),
        });
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "  one \n  ree");
        pane.undo();
        assert_eq!(pane.content.text(), "  one two\nthree");
        // Selected from the end of the indent, the indent goes down to the new line.
        pane.select(2..5);
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "\n   two\nthree");
    }

    #[test]
    fn backspace_in_an_indent_goes_back_to_the_last_stop() {
        let mut pane = Pane::new("      x\n");
        pane.content.move_to(cursor_at(0, 6));
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "    x\n");
        assert_eq!(pane.content.cursor(), cursor_at(0, 4));
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "x\n");
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(pane.content.text(), "    x\n", "each press is an undo step");
        // From one space, back to the start of the line.
        let mut pane = Pane::new("a\n x");
        pane.content.move_to(cursor_at(1, 1));
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "a\nx");
        // At the start of the line, it joins the line to the one above as usual.
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "ax");
    }

    #[test]
    fn backspace_after_text_or_over_a_selection_deletes_as_usual() {
        let mut pane = Pane::new("  é  ");
        pane.content.move_to(cursor_at(0, 6));
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "  é ");
        // A tab isn't an indent of spaces.
        let mut pane = Pane::new("\t\t");
        pane.content.move_to(cursor_at(0, 2));
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "\t");
        // A selection is deleted, and nothing else.
        let mut pane = Pane::new("        x");
        pane.content.move_to(Cursor {
            position: Position { line: 0, column: 7 },
            selection: Some(Position { line: 0, column: 6 }),
        });
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.content.text(), "       x");
        assert!(matches!(
            binding_for(Named::Backspace, Modifiers::empty()),
            Some(Binding::Custom(Message::Backspace))
        ));
    }

    #[test]
    fn backspace_in_an_indent_while_reviewing() {
        let mut pane = Pane::new("        one\ntwo");
        pane.suggest(12..15, "2").unwrap();
        pane.put_caret(8);
        assert_eq!(pane.backspace(4), Ok(true));
        assert_eq!(pane.model.text(), "    one\ntwo");
        assert_eq!(pane.suggestions()[0].range, 8..11);
        // In spaces a suggestion changes, it is refused and the caret stays put.
        let mut pane = Pane::new("        x");
        pane.suggest(5..6, "").unwrap();
        pane.put_caret(8);
        let cursor = pane.content.cursor();
        assert_eq!(pane.backspace(4), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.content.cursor(), cursor);
        assert_eq!(pane.model.text(), "        x");
    }

    #[test]
    fn ctrl_backspace_and_ctrl_delete_delete_a_word() {
        let mut pane = Pane::new("one two  three");
        pane.content.move_to(cursor_at(0, 14));
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(true));
        assert_eq!(pane.content.text(), "one two  ");
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(true));
        assert_eq!(pane.content.text(), "one ");
        pane.content.move_to(cursor_at(0, 0));
        assert_eq!(pane.delete_to(Motion::WordRight), Ok(true));
        assert_eq!(pane.content.text(), " ");
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(pane.content.text(), "one ", "each is one undo step");
        // Nothing before the start of the text, or after its end.
        pane.content.move_to(cursor_at(0, 0));
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(false));
        pane.content.move_to(cursor_at(0, 4));
        assert_eq!(pane.delete_to(Motion::WordRight), Ok(false));
        assert_eq!(pane.content.text(), "one ");
        // Words with multi-byte letters, and at the start of a line it joins the one above.
        let mut pane = Pane::new("naïve\ncafé au");
        pane.content.move_to(cursor_at(1, 5));
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(true));
        assert_eq!(pane.content.text(), "naïve\n au");
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(true));
        assert_eq!(pane.content.text(), "naïve au");
    }

    #[test]
    fn deleting_a_word_over_a_selection_deletes_only_the_selection() {
        let mut pane = Pane::new("one two three");
        pane.select(4..6);
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(true));
        assert_eq!(pane.content.text(), "one o three");
        // ⌘⌫ on macOS: to the start of the line.
        let mut pane = Pane::new("first\nsecond line");
        pane.content.move_to(cursor_at(1, 11));
        assert_eq!(pane.delete_to(Motion::Home), Ok(true));
        assert_eq!(pane.content.text(), "first\n");
    }

    #[test]
    fn deleting_a_word_while_reviewing() {
        let (mut pane, _, _) = reviewing_pane();
        // After "Stay", between the suggestions, the word goes and the suggestion after moves.
        pane.put_caret(14);
        assert_eq!(pane.delete_to(Motion::WordLeft), Ok(true));
        assert_eq!(pane.model.text(), "Be quick.  calm.");
        assert_eq!(pane.suggestions()[1].range, 11..15);
        // Right after a suggestion, the word before is its new text: refused, caret put back.
        pane.put_caret(15);
        let cursor = pane.content.cursor();
        assert_eq!(
            pane.delete_to(Motion::WordLeft),
            Err(IN_A_SUGGESTION.to_string())
        );
        assert_eq!(pane.content.cursor(), cursor);
        assert_eq!(pane.model.text(), "Be quick.  calm.");
    }

    #[test]
    fn the_word_keys_delete_by_word_or_line() {
        let jump = match cfg!(target_os = "macos") {
            true => Modifiers::ALT,
            false => Modifiers::CTRL,
        };
        let deletes_to = |key, modifiers| match binding_for(key, modifiers) {
            Some(Binding::Custom(Message::DeleteTo(motion))) => Some(motion),
            _ => None,
        };
        assert_eq!(deletes_to(Named::Backspace, jump), Some(Motion::WordLeft));
        assert_eq!(deletes_to(Named::Delete, jump), Some(Motion::WordRight));
        let command_backspace = deletes_to(Named::Backspace, Modifiers::LOGO);
        match cfg!(target_os = "macos") {
            true => assert_eq!(command_backspace, Some(Motion::Home)),
            false => assert_eq!(command_backspace, None),
        }
    }

    #[test]
    fn enter_while_reviewing_moves_suggestions_or_is_refused() {
        let mut pane = Pane::new("  Be quick.\n  Stay calm.");
        pane.suggest(19..23, "steady").unwrap();
        pane.put_caret(11);
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.model.text(), "  Be quick.\n  \n  Stay calm.");
        assert_eq!(pane.suggestions()[0].range, 22..26);
        // On a line of only spaces, they move down past the empty line left behind.
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.model.text(), "  Be quick.\n\n  \n  Stay calm.");
        assert_eq!(pane.suggestions()[0].range, 23..27);
        // Inside the text the suggestion changes, Enter is refused and the caret stays put.
        pane.put_caret(25);
        let (shown, cursor) = (pane.content.text(), pane.content.cursor());
        assert_eq!(pane.new_line(), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.content.text(), shown);
        assert_eq!(pane.content.cursor(), cursor);
        // So is one that would take the spaces a suggestion changes down with it.
        let mut pane = Pane::new("    x");
        pane.suggest(0..2, "").unwrap();
        pane.put_caret(4);
        let cursor = pane.content.cursor();
        assert_eq!(pane.new_line(), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.content.cursor(), cursor);
        assert_eq!(pane.model.text(), "    x");
    }
}
