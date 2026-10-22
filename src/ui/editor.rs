//! The text editing widget: the text column, Tab as spaces, Enter and Backspace that know about
//! indents (and Enter and Tab about Markdown lists, and in code about blocks and pairs), deleting
//! a word, moving, duplicating and deleting lines, Ctrl+D's next match, Ctrl+/'s comments,
//! Markdown's bold and italic markers, undo and redo (its colours come from `highlighter`). A
//! `Pane` pairs the widget's content with the text model behind it, and shows suggested edits
//! inside the text until each is accepted or rejected (review mode).

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::Motion;
use iced::widget::{container, stack};
use iced::{Background, Border, Element, Fill, Padding, Theme};

use crate::core::find::{find_all, word_at};
use crate::core::markdown::{
    BOLD, ITALIC, ListEnter, fence_enter, is_list_item, list_enter, toggle_marker, toggle_tasks,
};
use crate::core::review::{DisplayMap, Review, Shown, Suggestion, clashes};
use crate::core::scratch::clamp_column;
use crate::core::settings::{ReadingMode, Settings};
use crate::core::syntax::{Syntax, toggle_comments};
use crate::core::text_file::newlines_only;
use crate::core::text_model::{Change, TextModel, offset, position, positions, selected_lines};
use crate::core::typing::{enter, in_empty_pair, typed};
use crate::ui::app::Message;
use crate::ui::highlighter::{Colouring, Highlight, MarkupHighlighter, highlight_format};
use crate::ui::scroll_bar::scroll_bar;
use crate::ui::text_editor::{
    self, Action, Binding, Content, Cursor, Edit, KeyPress, PaintLook, PaintRun, Position, Status,
    TextEditor,
};
use crate::ui::theme::{CHAR_WIDTH_RATIO, Roles};

pub const EDITOR_ID: &str = "doc";
/// Why an edit to the text a suggestion changes was refused.
const IN_A_SUGGESTION: &str = "Accept or reject the suggestion there first";
/// Why Ctrl+D was refused.
const REVIEWING: &str = "Accept or reject the suggestions first";
pub const LINE_HEIGHT: f32 = 1.5;
/// Text starts near the top left, like any text editor, with just enough room to breathe.
pub const PADDING: Padding = Padding {
    top: 12.0,
    right: 16.0,
    bottom: 24.0,
    left: 16.0,
};

/// The pane's editor in its column, with `paint` (any suggestions, from `Pane::suggestion_runs`,
/// find matches and Ctrl+D's other places) drawn under the text, coloured for its `syntax`
/// (Markdown, a data file's keys and values, a shell script). Without `editable` the text is
/// dimmed; it can still be scrolled, selected and copied, and `App::update` drops any change to
/// it.
pub fn view<'a>(
    pane: &'a Pane,
    settings: &Settings,
    editable: bool,
    syntax: Syntax,
    paint: Vec<PaintRun>,
) -> Element<'a, Message> {
    let reviewing = pane.reviewing();
    let colouring = Colouring {
        syntax,
        markup: settings.colour_markup,
        review: pane.review_highlights(),
    };
    // Line numbers widen the left padding, so the text keeps its width.
    let gutter = match settings.line_numbers {
        true => text_editor::gutter_width(pane.content.line_count(), settings.font_size),
        false => 0.0,
    };
    let padding = Padding {
        left: PADDING.left + gutter,
        ..PADDING
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
        .padding(padding)
        .line_numbers(settings.line_numbers)
        .height(Fill)
        .style(move |theme, _status| style(theme, editable))
        .on_action(Message::Edit)
        .paint(paint)
        .highlight_with::<MarkupHighlighter>(colouring, highlight_format);

    let column = match settings.reading_mode {
        ReadingMode::Comfort { chars } => {
            let text_width = f32::from(chars) * settings.font_size * CHAR_WIDTH_RATIO;
            container(editor).max_width(text_width + padding.left + padding.right)
        }
        ReadingMode::FullWidth => container(editor),
    };
    // The scroll bar sits at the right edge, over the editor's right padding (or beside a
    // comfort-width column), and shows only when the text is taller than the view.
    let bar = scroll_bar(pane.content.scroll_rows(), |lines| {
        Message::Edit(Action::Scroll { lines })
    });
    stack![
        container(column).width(Fill).height(Fill),
        container(bar).align_right(Fill).height(Fill),
    ]
    .into()
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
/// starts a line indented like the caret's (in Markdown, `App::update` has Tab nest a list item and
/// Enter without Shift continue a list), Backspace in a line's leading spaces goes back to the last
/// indent stop, Ctrl+Backspace and Ctrl+Delete delete a word (⌥⌫ and ⌥⌦ on macOS, where ⌘⌫ and ⌘⌦
/// delete to the start and end of the line), Alt+↑ and Alt+↓ move lines, Ctrl+Shift+D and
/// Ctrl+Shift+K duplicate and delete them, Ctrl+D selects the next match, Ctrl+/ comments lines,
/// Ctrl+B and Ctrl+I toggle bold and italic markers (`App::update` drops them outside Markdown),
/// Ctrl+Z undoes, Ctrl+Shift+Z and Ctrl+Y redo, and the app's own Ctrl shortcuts are left for the
/// app. A pane that isn't `editable` keeps only iced's own keys (moving, selecting, copying).
/// While `reviewing`, Ctrl+Enter and Ctrl+Backspace are left to the app (they accept and reject),
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
                "p" | "P"
                    | "0"
                    | ","
                    | "l"
                    | "w"
                    | "W"
                    | "s"
                    | "S"
                    | "o"
                    | "n"
                    | "f"
                    | "F"
                    | "g"
                    | "="
                    | "+"
                    | "-"
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
    if reviewing && decides(&press) {
        return None;
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
            Key::Character("b" | "B") => Some(Binding::Custom(Message::ToggleMarker(BOLD))),
            Key::Character("i" | "I") => Some(Binding::Custom(Message::ToggleMarker(ITALIC))),
            Key::Character("/") => Some(Binding::Custom(Message::ToggleComment)),
            Key::Character("d" | "D") if press.modifiers.shift() => {
                Some(Binding::Custom(Message::DuplicateLines))
            }
            Key::Character("d") => Some(Binding::Custom(Message::SelectNextMatch)),
            Key::Character("k" | "K") if press.modifiers.shift() => {
                Some(Binding::Custom(Message::DeleteLines))
            }
            Key::Named(Named::Enter) if press.modifiers.shift() => {
                Some(Binding::Custom(Message::ToggleTasks))
            }
            _ => Binding::from_key_press(press),
        };
    }
    match press.key.as_ref() {
        Key::Named(Named::Tab) if press.modifiers.shift() => {
            Some(Binding::Custom(Message::Unindent))
        }
        Key::Named(Named::Tab) => Some(Binding::Custom(Message::Indent)),
        Key::Named(Named::Enter) => Some(Binding::Custom(Message::NewLine {
            shift: press.modifiers.shift(),
        })),
        Key::Named(Named::Backspace) => Some(Binding::Custom(Message::Backspace)),
        Key::Named(arrow @ (Named::ArrowUp | Named::ArrowDown))
            if press.modifiers == Modifiers::ALT =>
        {
            let up = arrow == Named::ArrowUp;
            Some(Binding::Custom(Message::MoveLines { up }))
        }
        _ => Binding::from_key_press(press),
    }
}

/// Whether a key press is Ctrl+Enter, or Ctrl+Backspace with or without Shift, which accept and
/// reject suggestions while any wait. Text boxes leave them to `App`'s shortcuts then.
/// Ctrl+Shift+Enter isn't one: it ticks Markdown task boxes (Matt dropped accepting them all).
pub fn decides(press: &KeyPress) -> bool {
    press.modifiers.command()
        && match press.key.as_ref() {
            Key::Named(Named::Enter) => !press.modifiers.shift(),
            Key::Named(Named::Backspace) => true,
            _ => false,
        }
}

/// What to do with suggested edits: accept or reject one (by id; `None` is the current one, see
/// `Pane::current_suggestion`), or all of them.
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
    /// The places Ctrl+D selected besides the widget's own selection, as byte ranges of the
    /// model's text in order. Typing changes them all; then each is a caret (an empty range).
    /// Moving the caret, or any other edit, lets them go.
    others: Vec<Range<usize>>,
}

impl Pane {
    pub fn new(text: &str) -> Pane {
        Pane {
            content: Content::with_text(text),
            model: TextModel::new(text.to_string()),
            review: Review::default(),
            display: None,
            others: Vec::new(),
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
        let caret = self.caret();
        self.others.clear();
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
        let current = self.current_suggestion();
        let caret = self.caret();
        let chosen: Vec<Suggestion> = match decision {
            Decision::Accept(id) | Decision::Reject(id) => id
                .or(current)
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
        match changes.last() {
            Some(last) => self.put_caret(last.at + last.inserted.len()),
            None => self.put_caret(caret),
        }
        changed
    }

    /// Where the caret is, as a byte offset of the model's text.
    pub fn caret(&self) -> usize {
        self.to_text(self.content.cursor().position)
    }

    /// The caret and the selection's other end, as byte offsets of the model's text; both are
    /// the caret when nothing is selected.
    fn caret_and_anchor(&self) -> (usize, usize) {
        let cursor = self.content.cursor();
        let caret = self.to_text(cursor.position);
        (caret, cursor.selection.map_or(caret, |at| self.to_text(at)))
    }

    /// The selection, or an empty range at the caret when nothing is selected.
    fn selection_or_caret(&self) -> Range<usize> {
        let caret = self.caret();
        self.selection().unwrap_or(caret..caret)
    }

    /// The suggestion Ctrl+Enter and Ctrl+Backspace decide: the one the caret is on (in or
    /// touching the text it takes out or puts in), or else the first in the text.
    pub fn current_suggestion(&self) -> Option<u64> {
        let caret = self.content.cursor().position;
        let display = self.display.as_ref()?;
        display
            .suggestion_at(offset(&display.text, caret.line, caret.column))
            .or_else(|| self.suggestions().first().map(|s| s.id))
    }

    /// Where widget position `at` is in the model's text. Inside a suggestion's added text,
    /// that's where the suggestion sits.
    fn to_text(&self, at: Position) -> usize {
        let shown_offset = offset(self.shown(), at.line, at.column);
        match &self.display {
            Some(display) => display.to_text(shown_offset),
            None => shown_offset,
        }
    }

    /// Puts the caret at byte `offset` of the model's text, with nothing selected.
    fn put_caret(&mut self, offset: usize) {
        self.others.clear();
        let position = self.position(offset, true);
        self.content.move_to(Cursor {
            position,
            selection: None,
        });
    }

    /// Where byte `offset` of the model's text is in the widget. In review mode an offset right
    /// at a suggestion's added text goes before it, or after it when `after` is set.
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

    /// Ctrl+G: puts the caret at the start of `line` (counted from 0), or at the end of the text
    /// past its last line, which scrolls it into view.
    pub fn go_to_line(&mut self, line: usize) {
        self.put_caret(offset(self.model.text(), line, 0));
    }

    /// Selects byte range `range` of the model's text with the caret at its end, which scrolls
    /// it into view.
    pub fn select(&mut self, range: Range<usize>) {
        self.others.clear();
        let start = self.position(range.start, false);
        let end = self.position(range.end, false);
        self.content.move_to(Cursor {
            position: end,
            selection: Some(start),
        });
    }

    /// The suggestions as the editor draws them: the text they take out on red and struck
    /// through, the text they put in on green.
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
            let (look, color) = match piece.shown {
                Shown::Text => continue,
                Shown::Removed => (PaintLook::Removed(roles.fg_muted), roles.removed),
                Shown::Added => (PaintLook::Added, roles.added),
            };
            runs.push(PaintRun {
                start: at(piece.display.start),
                end: at(piece.display.end),
                color,
                look,
            });
        }
        runs
    }

    /// The text suggestions take out and put in, one entry per line, for the highlighter.
    fn review_highlights(&self) -> Vec<(usize, Range<usize>, Highlight)> {
        let Some(display) = &self.display else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for piece in &display.pieces {
            let highlight = match piece.shown {
                Shown::Removed | Shown::Added => Highlight::Suggested,
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
    /// the text they change (or to the text they put in) is refused, with the reason.
    pub fn perform(&mut self, action: Action) -> Result<bool, String> {
        if !action.is_edit() {
            if !matches!(action, Action::Scroll { .. }) {
                self.others.clear();
            }
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
        if let Action::Edit(edit) = &action
            && !self.others.is_empty()
        {
            return self.edit_every(edit.clone());
        }
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
        // Typed right before a pure insert's added text (`at` is where the insert sits too), the
        // insert moves along so the typing lands at the caret. Not at the end of the display,
        // which is after the last suggestion's added text.
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

    /// Ctrl+D. With nothing selected it selects the word at the caret. With text selected it
    /// selects the next place the same text appears too (matching case, and whole words for a
    /// word, round to the top after the end), so typing then changes every selected place at
    /// once. Refused while suggestions
    /// wait. Returns whether anything more was selected, or why it can't.
    pub fn select_next_match(&mut self) -> Result<bool, String> {
        if self.reviewing() {
            return Err(REVIEWING.to_string());
        }
        let text = self.model.text();
        let Some(selected) = self.selection() else {
            let Some(word) = word_at(text, self.caret()) else {
                return Ok(false);
            };
            self.select(word);
            return Ok(true);
        };
        let taken = |range: &Range<usize>| *range == selected || self.others.contains(range);
        // A whole word selected matches whole words, so "cat" skips "catalog".
        let whole_word = word_at(text, selected.start) == Some(selected.clone());
        let matches = find_all(text, &text[selected.clone()], true, whole_word);
        let mut free = matches.iter().filter(|range| !taken(range));
        let next = match free.clone().find(|range| range.start >= selected.end) {
            Some(next) => next.clone(),
            None => match free.next() {
                Some(first) => first.clone(),
                None => return Ok(false),
            },
        };
        let mut others = std::mem::take(&mut self.others);
        others.push(selected);
        others.sort_by_key(|range| range.start);
        self.select(next);
        self.others = others;
        Ok(true)
    }

    /// The places Ctrl+D selected besides the widget's own, as the editor draws them: a caret at
    /// the end of each, over a fill for one that holds text.
    pub fn other_runs(&self, roles: &Roles) -> Vec<PaintRun> {
        let mut runs = Vec::new();
        for range in &self.others {
            let (start, end) = (
                self.position(range.start, false),
                self.position(range.end, true),
            );
            if !range.is_empty() {
                let look = PaintLook::Found { outline: None };
                let color = roles.accent_cool;
                runs.push(PaintRun {
                    start,
                    end,
                    color,
                    look,
                });
            }
            let (start, color, look) = (end, roles.fg, PaintLook::Caret);
            runs.push(PaintRun {
                start,
                end,
                color,
                look,
            });
        }
        runs
    }

    /// Lets go of the places Ctrl+D selected, keeping the widget's own selection (Esc).
    pub fn clear_others(&mut self) {
        self.others.clear();
    }

    /// `edit`, made while Ctrl+D has several places selected: the same edit at each, as one undo
    /// step, after which each is a caret after its new text. Backspace and Delete at a caret take
    /// the character before or after it. Indent and Unindent let the other places go and act on
    /// the widget's own selection alone. Returns whether the text changed.
    fn edit_every(&mut self, edit: Edit) -> Result<bool, String> {
        let typed = match &edit {
            Edit::Insert(c) => c.to_string(),
            Edit::Paste(pasted) => pasted.to_string(),
            Edit::Enter => "\n".to_string(),
            Edit::Backspace | Edit::Delete => String::new(),
            Edit::Indent | Edit::Unindent => {
                self.others.clear();
                return self.perform(Action::Edit(edit));
            }
        };
        let caret = self.caret();
        let main = self.selection().unwrap_or(caret..caret);
        let text = self.model.text();
        // Each place, and whether it's the widget's own.
        let mut places: Vec<(Range<usize>, bool)> = self
            .others
            .iter()
            .map(|range| (range.clone(), false))
            .collect();
        places.push((main, true));
        places.sort_by_key(|(range, _)| range.start);
        for (range, _) in &mut places {
            match edit {
                Edit::Backspace if range.start == range.end => {
                    range.start -= text[..range.start]
                        .chars()
                        .next_back()
                        .map_or(0, char::len_utf8);
                }
                Edit::Delete if range.start == range.end => {
                    range.end += text[range.end..].chars().next().map_or(0, char::len_utf8);
                }
                _ => {}
            }
        }
        // Carets side by side can now overlap: merge them, keeping which one is the widget's.
        let mut merged: Vec<(Range<usize>, bool)> = Vec::new();
        for (range, main) in places {
            match merged.last_mut() {
                Some((last, last_main)) if range.start < last.end => {
                    last.end = last.end.max(range.end);
                    *last_main |= main;
                }
                _ => merged.push((range, main)),
            }
        }
        let edits: Vec<(Range<usize>, &str)> = merged
            .iter()
            .filter(|(range, _)| !(range.is_empty() && typed.is_empty()))
            .map(|(range, _)| (range.clone(), typed.as_str()))
            .collect();
        let changes = self.model.replace_each(&edits);
        let changed = self.replay(&changes);
        // Where each place's caret is now: after its new text, moved by the edits before it.
        let (mut moved, mut main_caret, mut others) = (0isize, 0, Vec::new());
        for (range, main) in &merged {
            let at = range.start.saturating_add_signed(moved) + typed.len();
            moved += typed.len() as isize - range.len() as isize;
            match main {
                true => main_caret = at,
                false => others.push(at..at),
            }
        }
        // Carets that met are one caret now.
        others.dedup();
        others.retain(|caret| caret.start != main_caret);
        self.put_caret(main_caret);
        self.others = others;
        Ok(changed)
    }

    /// The selected text as a byte range of the model's text, or `None` when nothing is
    /// selected. In review mode a selection of only added text selects nothing.
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

    /// Replace all: puts `text` in place of each of `ranges` (byte ranges of the model's text, in
    /// order, not overlapping) as one undo step. Refused if a suggestion is on any of them.
    pub fn replace_all(&mut self, ranges: &[Range<usize>], text: &str) -> Result<bool, String> {
        let suggestions = self.suggestions();
        if ranges
            .iter()
            .any(|range| suggestions.iter().any(|s| clashes(&s.range, range)))
        {
            return Err(IN_A_SUGGESTION.to_string());
        }
        let edits: Vec<(Range<usize>, &str)> =
            ranges.iter().map(|range| (range.clone(), text)).collect();
        let changes = self.model.replace_each(&edits);
        Ok(self.replay(&changes))
    }

    /// Replaces `range` (byte offsets) with `text` as one undo step, such as a reload from disk.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let change = self.model.replace(range, text);
        self.replay(&[change]);
    }

    /// Tab: adds one indent (`tab_spaces` spaces) to the start of each line the selection is on.
    /// With nothing selected, or a selection within one line, it types the spaces instead, except
    /// on a list item in a Markdown file (`markdown`), which it nests by indenting its whole line.
    /// Returns whether the text changed, or why it can't.
    pub fn indent(&mut self, tab_spaces: usize, markdown: bool) -> Result<bool, String> {
        self.reindent(tab_spaces, true, markdown)
    }

    /// Shift+Tab: removes up to one indent (`tab_spaces` spaces) from the start of the caret's
    /// line, or of each line the selection is on. Returns whether the text changed, or why it
    /// can't.
    pub fn unindent(&mut self, tab_spaces: usize) -> Result<bool, String> {
        self.reindent(tab_spaces, false, false)
    }

    /// Enter (and Shift+Enter): a new line in place of any selection, indented like the caret's
    /// line up to the caret. When only spaces are before the caret they move down with it, so the
    /// line left behind is empty rather than trailing spaces. Returns whether the text changed,
    /// or why it can't.
    pub fn new_line(&mut self) -> Result<bool, String> {
        if !self.others.is_empty() {
            return self.perform(Action::Edit(Edit::Enter));
        }
        let cursor = self.content.cursor();
        let range = self.selection_or_caret();
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

    /// Enter in a Markdown file. On a list item it starts the next item (the same bullet or the
    /// next number, and an empty task box if it had one), and on a quote line it continues the
    /// quote. On an empty item it takes off one layer instead: an indented item unindents, any
    /// other loses its marker, and a quote line with nothing else loses its quote markers. At the
    /// end of a fence that opens a code block, it puts the closing fence in two lines down, with
    /// the caret between. Anywhere else, in fenced code, or with the caret in the line's prefix,
    /// it is `new_line`.
    /// Returns whether the text changed, or why it can't.
    pub fn markdown_new_line(&mut self, tab_spaces: usize) -> Result<bool, String> {
        if !self.others.is_empty() {
            return self.new_line();
        }
        let cursor = self.content.cursor();
        let range = self.selection_or_caret();
        if range.is_empty()
            && let Some((typed, at)) = fence_enter(self.model.text(), range.start)
        {
            let edited = self.perform(Action::Edit(Edit::Paste(Arc::new(typed))))?;
            self.put_caret(at);
            return Ok(edited);
        }
        match list_enter(self.model.text(), range.start) {
            Some(ListEnter::Continue(prefix)) => {
                let typed = Edit::Paste(Arc::new(format!("\n{prefix}")));
                self.perform(Action::Edit(typed))
            }
            // With text selected, an empty item gets a plain new line in place of it.
            Some(ListEnter::Unindent) if range.is_empty() => self.unindent(tab_spaces),
            Some(ListEnter::Delete(peeled)) if range.is_empty() => {
                self.select(peeled);
                self.perform_or_put_back(Edit::Delete, cursor)
            }
            _ => self.new_line(),
        }
    }

    /// Enter in code (a shell script, JSON or YAML): indents after a line that opens a block,
    /// opens up a pair of brackets the caret is between, and lines up a line that ends a block
    /// (see `typing::enter`). Anywhere else it is `new_line`. Returns whether the text changed,
    /// or why it can't.
    pub fn code_new_line(&mut self, tab_spaces: usize, syntax: Syntax) -> Result<bool, String> {
        if !self.others.is_empty() {
            return self.new_line();
        }
        let cursor = self.content.cursor();
        let range = self.selection_or_caret();
        let Some((replaced, with, caret)) = enter(self.model.text(), range, tab_spaces, syntax)
        else {
            return self.new_line();
        };
        self.select(replaced);
        let edited = self.perform_or_put_back(Edit::Paste(Arc::new(with)), cursor)?;
        self.put_caret(caret);
        Ok(edited)
    }

    /// Typing `typed` in code (a shell script or JSON): brackets and quotes close themselves, wrap
    /// a selection, and step over their closer (see `typing::typed`). Anything else, or with
    /// Ctrl+D's other places, is typed as usual. Returns whether the text changed, or why it
    /// can't.
    pub fn type_in_code(&mut self, typed_char: char, syntax: Syntax) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let selection = self.selection_or_caret();
        let paired = match self.others.is_empty() {
            true => typed(self.model.text(), selection.clone(), typed_char, syntax),
            false => None,
        };
        let Some((replaced, with, selected)) = paired else {
            return self.perform(Action::Edit(Edit::Insert(typed_char)));
        };
        // Stepping over a closer changes nothing; the caret just moves past it.
        let mut edited = false;
        if self.model.text()[replaced.clone()] != with {
            if replaced != selection {
                self.select(replaced);
            }
            edited = self.perform_or_put_back(Edit::Paste(Arc::new(with)), cursor)?;
        }
        match selected.is_empty() {
            true => self.put_caret(selected.start),
            false => self.select(selected),
        }
        Ok(edited)
    }

    /// Ctrl+B and Ctrl+I in a Markdown file: puts `marker` (`**`, or `_` or inside a word `*`
    /// for italic) around the selection and keeps the same text selected, or with nothing
    /// selected puts in a pair with the caret between. When the markers are already there, just
    /// outside the selection or at its ends, it takes them off instead. Returns whether the text
    /// changed, or why it can't.
    pub fn toggle_marker(&mut self, marker: &str) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let selection = self.selection_or_caret();
        let (replaced, with, selected) =
            toggle_marker(self.model.text(), selection.clone(), marker);
        if replaced != selection {
            self.select(replaced);
        }
        let edit = match with.is_empty() {
            true => Edit::Delete,
            false => Edit::Paste(Arc::new(with)),
        };
        let edited = self.perform_or_put_back(edit, cursor)?;
        match selected.is_empty() {
            true => self.put_caret(selected.start),
            false => self.select(selected),
        }
        Ok(edited)
    }

    /// Backspace. In the spaces at the start of a line it deletes back to the previous indent
    /// stop (a multiple of `tab_spaces`), so one press takes back a Tab; in code (`syntax`)
    /// between an empty pair, such as `(|)`, it deletes both; anywhere else, or with a selection,
    /// it deletes as usual. Returns whether the text changed, or why it can't.
    pub fn backspace(&mut self, tab_spaces: usize, syntax: Syntax) -> Result<bool, String> {
        if !self.others.is_empty() {
            return self.perform(Action::Edit(Edit::Backspace));
        }
        let cursor = self.content.cursor();
        let caret = self.caret();
        let text = self.model.text();
        let line_start = text[..caret].rfind('\n').map_or(0, |at| at + 1);
        let column = caret - line_start;
        let in_indent = column > 0 && text[line_start..caret].bytes().all(|byte| byte == b' ');
        if self.selection().is_none() && in_empty_pair(text, caret, syntax) {
            let opener = text[..caret].chars().next_back().map_or(0, char::len_utf8);
            let closer = text[caret..].chars().next().map_or(0, char::len_utf8);
            self.select(caret - opener..caret + closer);
            return self.perform_or_put_back(Edit::Delete, cursor);
        }
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

    /// Alt+↑ and Alt+↓: moves the caret's line, or every line the selection is on, up (`up`) or
    /// down past the next line, as one undo step, with the caret and selection on the same text.
    /// Nothing moves at the top or bottom, and lines with a suggestion on them, or on the line
    /// they would pass, are refused. Returns whether the text changed, or why it can't.
    pub fn move_lines(&mut self, up: bool) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let (caret, anchor) = self.caret_and_anchor();
        let text = self.model.text();
        let (first, last) = selected_lines(text, caret, anchor);
        let (top, bottom) = match up {
            true => (first.saturating_sub(1), last),
            false => (first, last + 1),
        };
        self.lines_free(top, bottom)?;
        let (caret, anchor) = (position(text, caret), position(text, anchor));
        let changes = self.model.move_lines(first..=last, up);
        if !self.replay(&changes) {
            return Ok(false);
        }
        let down = if up { -1 } else { 1 };
        self.put_moved(caret, cursor.selection.map(|_| anchor), down);
        Ok(true)
    }

    /// Ctrl+Shift+D: puts a copy of the caret's line, or of every line the selection is on, just
    /// below it, as one undo step, with the caret and selection moving onto the copy. Lines with a
    /// suggestion on them are refused. Returns whether the text changed, or why it can't.
    pub fn duplicate_lines(&mut self) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let (caret, anchor) = self.caret_and_anchor();
        let text = self.model.text();
        let (first, last) = selected_lines(text, caret, anchor);
        self.lines_free(first, last)?;
        let (caret, anchor) = (position(text, caret), position(text, anchor));
        let changes = self.model.duplicate_lines(first..=last);
        self.replay(&changes);
        let down = (last - first + 1) as isize;
        self.put_moved(caret, cursor.selection.map(|_| anchor), down);
        Ok(true)
    }

    /// Ctrl+Shift+K: deletes the caret's line, or every line the selection is on, as one undo
    /// step. The caret goes to the line that takes their place, at the same column where it can.
    /// Lines with a suggestion on them are refused. Returns whether the text changed, or why it
    /// can't.
    pub fn delete_lines(&mut self) -> Result<bool, String> {
        let (caret, anchor) = self.caret_and_anchor();
        let text = self.model.text();
        let (first, last) = selected_lines(text, caret, anchor);
        self.lines_free(first, last)?;
        let (_, column) = position(text, caret);
        let changes = self.model.delete_lines(first..=last);
        if !self.replay(&changes) {
            return Ok(false);
        }
        let text = self.model.text();
        // After the last line goes, the line above comes up to take its place.
        let line = first.min(text.matches('\n').count());
        let start = offset(text, line, 0);
        let length = text[start..].find('\n').unwrap_or(text.len() - start);
        self.put_caret(start + clamp_column(&text[start..start + length], column));
        Ok(true)
    }

    /// Puts the caret at (line, column) `caret` and any selection's other end at `anchor`, each
    /// `down` lines further on: where they are now on lines that moved, which kept their text.
    fn put_moved(&mut self, caret: (usize, usize), anchor: Option<(usize, usize)>, down: isize) {
        let moved = |(line, column): (usize, usize)| {
            let line = line.saturating_add_signed(down);
            self.position(offset(self.model.text(), line, column), true)
        };
        let position = moved(caret);
        let selection = anchor.map(moved);
        self.content.move_to(Cursor {
            position,
            selection,
        });
    }

    /// Refuses a line command (indent, move, duplicate, delete, comment) on the model's lines
    /// `first` to `last` when a suggestion is on any of them.
    fn lines_free(&self, first: usize, last: usize) -> Result<(), String> {
        let text = self.model.text();
        let lines = offset(text, first, 0)..offset(text, last + 1, 0);
        match self.suggestions().iter().any(|s| clashes(&s.range, &lines)) {
            true => Err(IN_A_SUGGESTION.to_string()),
            false => Ok(()),
        }
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
    fn reindent(&mut self, tab_spaces: usize, add: bool, markdown: bool) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let (caret, anchor) = self.caret_and_anchor();
        let (start, end) = (caret.min(anchor), caret.max(anchor));
        let text = self.model.text();
        let one_line = !text[start..end].contains('\n');
        let nest = add && markdown && one_line && is_list_item(text, start);
        if add && one_line && !nest {
            let spaces = Arc::new(" ".repeat(tab_spaces));
            return self.perform(Action::Edit(Edit::Paste(spaces)));
        }
        let (first, last) = selected_lines(text, start, end);
        self.lines_free(first, last)?;
        let changes = match add {
            true => self.model.indent(first..=last, tab_spaces),
            false => self.model.unindent(first..=last, tab_spaces),
        };
        if !self.replay(&changes) {
            return Ok(false);
        }
        self.follow(&changes, caret, cursor.selection.map(|_| anchor), nest);
        Ok(true)
    }

    /// Ctrl+/ in a script or config file: comments out the caret's line, or every line the
    /// selection is on, with `marker` (see `syntax::toggle_comments`), or uncomments them when
    /// they all are already, as one undo step. Lines with a suggestion on them are refused.
    /// Returns whether the text changed, or why it can't.
    pub fn toggle_comment(&mut self, marker: &str) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let (caret, anchor) = self.caret_and_anchor();
        let text = self.model.text();
        let (first, last) = selected_lines(text, caret, anchor);
        self.lines_free(first, last)?;
        let edits = toggle_comments(text, first..=last, marker);
        let edits: Vec<_> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        let changes = self.model.replace_each(&edits);
        if !self.replay(&changes) {
            return Ok(false);
        }
        self.follow(&changes, caret, cursor.selection.map(|_| anchor), false);
        Ok(true)
    }

    /// Ctrl+Shift+Enter in a Markdown file: ticks or unticks the task boxes on the caret's line, or
    /// every line the selection is on, or gives them boxes (see `markdown::toggle_tasks`), as one
    /// undo step. Lines with a suggestion on them are refused. Returns whether the text changed,
    /// or why it can't.
    pub fn toggle_tasks(&mut self) -> Result<bool, String> {
        let cursor = self.content.cursor();
        let (caret, anchor) = self.caret_and_anchor();
        let text = self.model.text();
        let (first, last) = selected_lines(text, caret, anchor);
        self.lines_free(first, last)?;
        let edits = toggle_tasks(text, first..=last);
        let edits: Vec<_> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        let changes = self.model.replace_each(&edits);
        if !self.replay(&changes) {
            return Ok(false);
        }
        self.follow(&changes, caret, cursor.selection.map(|_| anchor), false);
        Ok(true)
    }

    /// After `changes` put in or took out text at the start of lines, puts the caret and any
    /// selection's other end (`caret` and `anchor`, offsets from before them) back on the same
    /// text: what is after a change moves with it, and the start of the line stays put, unless
    /// `whole` (a nested list item, which moves as a whole).
    fn follow(&mut self, changes: &[Change], caret: usize, anchor: Option<usize>, whole: bool) {
        let moved = |mut at: usize| {
            for change in changes {
                if at > change.at || whole {
                    let removed_end = change.at + change.removed.len();
                    at = at.max(removed_end) - change.removed.len() + change.inserted.len();
                }
            }
            at
        };
        let position = self.position(moved(caret), true);
        let selection = anchor.map(|anchor| self.position(moved(anchor), true));
        self.content.move_to(Cursor {
            position,
            selection,
        });
    }

    /// Copies changes the model made into the widget, in order. This keeps the scroll position
    /// and leaves the caret at the end of the last change. In review mode the suggestions move
    /// with the changes (one a change touches is dropped) and the widget shows the result.
    /// Returns whether there were any changes.
    fn replay(&mut self, changes: &[Change]) -> bool {
        self.others.clear();
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
            text.replace_range(change.removed_range(), &change.inserted);
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
    use iced::keyboard::key::{Code, NativeCode, Physical};

    /// Selects from (line, column) `anchor` to `caret`, as dragging the mouse there does.
    fn select_from(pane: &mut Pane, anchor: (usize, usize), caret: (usize, usize)) {
        let at = |(line, column)| Position { line, column };
        pane.content.move_to(Cursor {
            position: at(caret),
            selection: Some(at(anchor)),
        });
    }

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
        select_from(&mut pane, (0, 1), (2, 2));
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
        select_from(&mut pane, (0, 9), (0, 2));
        assert_eq!(pane.selection(), Some(2..9));
        // Selected the other way, it's the same range.
        select_from(&mut pane, (0, 2), (0, 9));
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
        assert!(pane.reviewing());
        assert_eq!(
            pane.content.text(),
            "Be quick.Take your time. Stay calmsteady."
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
        // Right after the first suggestion's added text.
        pane.put_caret(9);
        for c in " Now".chars() {
            assert_eq!(pane.perform(Action::Edit(Edit::Insert(c))), Ok(true));
        }
        assert_eq!(pane.model.text(), "Be quick. Now Stay calm.");
        assert_eq!(
            pane.content.text(),
            "Be quick.Take your time. Now Stay calmsteady."
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
            "Be quick.Take your time. Stay calmsteady."
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
    fn typing_right_before_an_insert_goes_before_its_added_text() {
        let mut pane = Pane::new("ab cd");
        pane.suggest(2..2, "XY").unwrap();
        pane.content.move_to(cursor_at(0, 2));
        assert_eq!(pane.perform(Action::Edit(Edit::Insert('!'))), Ok(true));
        assert_eq!(pane.content.text(), "ab!XY cd");
        assert_eq!(pane.content.cursor(), cursor_at(0, 3));
        assert!(pane.decide(Decision::AcceptAll));
        assert_eq!(pane.model.text(), "ab!XY cd");
        // After an insert at the end of the text, typing still goes after it.
        let mut pane = Pane::new("Hi.");
        pane.suggest(3..3, " Bye.").unwrap();
        pane.put_caret(3);
        assert_eq!(pane.perform(Action::Edit(Edit::Insert('!'))), Ok(true));
        assert_eq!(pane.content.text(), "Hi. Bye.!");
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
        assert!(pane.decide(Decision::Accept(Some(first))));
        assert_eq!(pane.model.text(), "Take your time. Stay calm.");
        assert_eq!(pane.content.text(), "Take your time. Stay calmsteady.");
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
    fn the_current_suggestion_is_the_one_at_the_caret_or_else_the_first() {
        let (mut pane, first, second) = reviewing_pane();
        // In " Stay", away from both suggestions, the first one is current.
        pane.put_caret(11);
        assert_eq!(pane.current_suggestion(), Some(first));
        // On "calm", that one is: accepting with no id takes it.
        pane.put_caret(16);
        assert_eq!(pane.current_suggestion(), Some(second));
        assert!(pane.decide(Decision::Accept(None)));
        assert_eq!(pane.model.text(), "Be quick. Stay steady.");
        // Rejecting with no id, the caret anywhere, takes the one left.
        pane.put_caret(20);
        assert!(!pane.decide(Decision::Reject(None)));
        assert!(!pane.reviewing());
        assert_eq!(pane.current_suggestion(), None);
        assert_eq!(pane.content.text(), "Be quick. Stay steady.");
    }

    #[test]
    fn ctrl_enter_and_ctrl_backspace_are_left_to_the_app_while_reviewing() {
        let press = |key: Named, modifiers: Modifiers| KeyPress {
            key: Key::Named(key),
            modified_key: Key::Named(key),
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        for (key, modifiers) in [
            (Named::Enter, Modifiers::COMMAND),
            (Named::Backspace, Modifiers::COMMAND),
            (Named::Backspace, Modifiers::COMMAND | Modifiers::SHIFT),
        ] {
            assert!(decides(&press(key, modifiers)));
            assert!(key_binding(press(key, modifiers), true, true).is_none());
        }
        assert!(!decides(&press(Named::Enter, Modifiers::empty())));
        assert!(!decides(&press(
            Named::Enter,
            Modifiers::COMMAND | Modifiers::SHIFT
        )));
        // Not reviewing, Ctrl+Backspace still deletes a word.
        assert!(key_binding(press(Named::Backspace, Modifiers::COMMAND), true, false).is_some());
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
    fn ctrl_z_takes_back_one_accepted_suggestion() {
        let (mut pane, first, _) = reviewing_pane();
        assert!(pane.decide(Decision::Accept(Some(first))));
        assert_eq!(pane.model.text(), "Take your time. Stay calm.");
        assert!(pane.undo());
        assert_eq!(pane.model.text(), "Be quick. Stay calm.");
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
        // " Stay " starts after the first suggestion's added text.
        let stay = pane.position(9, true);
        assert_eq!(&pane.content.text()[shown_offset(stay)..][..5], " Stay");
        assert_eq!(shown_offset(pane.position(9, false)), 9);
        // Selecting from inside "quick" to inside "Stay" selects that text of the model.
        pane.content.move_to(Cursor {
            position: pane.position(12, true),
            selection: Some(pane.position(4, true)),
        });
        assert_eq!(pane.selection(), Some(4..12));
        // A selection of only added text is no selection.
        select_from(&mut pane, (0, 12), (0, 17));
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
        // `Clip` copies the model's text under the selection, without the added text.
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
        select_from(&mut pane, (0, 4), (1, 3));
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
        select_from(pane, (0, 0), (2, 0));
    }

    #[test]
    fn tab_indents_each_selected_line_as_one_step() {
        let mut pane = Pane::new("alpha\nbeta\ngamma\n");
        select_two_lines(&mut pane);
        assert_eq!(pane.indent(4, false), Ok(true));
        assert_eq!(pane.content.text(), "    alpha\n    beta\ngamma\n");
        assert_eq!(pane.model.text(), pane.content.text());
        // The same lines stay selected, indents included.
        assert_eq!(pane.selection(), Some(0..19));
        assert_eq!(pane.indent(4, false), Ok(true), "again, a level deeper");
        assert_eq!(pane.content.text(), "        alpha\n        beta\ngamma\n");
        pane.undo();
        assert_eq!(pane.content.text(), "    alpha\n    beta\ngamma\n");

        // A selection that starts and ends inside lines keeps its ends on the same letters.
        select_from(&mut pane, (0, 5), (1, 6));
        assert_eq!(pane.indent(2, false), Ok(true));
        assert_eq!(pane.content.text(), "      alpha\n      beta\ngamma\n");
        assert_eq!(pane.copied().as_deref(), Some("lpha\n      be"));
    }

    #[test]
    fn tab_within_one_line_types_spaces() {
        let mut pane = Pane::new("one two");
        pane.content.move_to(cursor_at(0, 3));
        assert_eq!(pane.indent(4, false), Ok(true));
        assert_eq!(pane.content.text(), "one     two");
        // A selection within one line is replaced, as typing would.
        select_from(&mut pane, (0, 0), (0, 3));
        assert_eq!(pane.indent(2, false), Ok(true));
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
    fn enter_and_shift_enter_start_a_new_line_told_apart() {
        // Only Enter continues a Markdown list, so the message says which it was.
        for modifiers in [Modifiers::empty(), Modifiers::SHIFT] {
            assert!(matches!(
                binding_for(Named::Enter, modifiers),
                Some(Binding::Custom(Message::NewLine { shift })) if shift == modifiers.shift()
            ));
        }
    }

    #[test]
    fn ctrl_shift_enter_ticks_tasks_even_while_suggestions_wait() {
        let press = KeyPress {
            key: Key::Named(Named::Enter),
            modified_key: Key::Named(Named::Enter),
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        for reviewing in [false, true] {
            assert!(matches!(
                key_binding(press.clone(), true, reviewing),
                Some(Binding::Custom(Message::ToggleTasks))
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
        select_from(&mut pane, (0, 6), (1, 2));
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
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "    x\n");
        assert_eq!(pane.content.cursor(), cursor_at(0, 4));
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "x\n");
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(pane.content.text(), "    x\n", "each press is an undo step");
        // From one space, back to the start of the line.
        let mut pane = Pane::new("a\n x");
        pane.content.move_to(cursor_at(1, 1));
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "a\nx");
        // At the start of the line, it joins the line to the one above as usual.
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "ax");
    }

    #[test]
    fn backspace_after_text_or_over_a_selection_deletes_as_usual() {
        let mut pane = Pane::new("  é  ");
        pane.content.move_to(cursor_at(0, 6));
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "  é ");
        // A tab isn't an indent of spaces.
        let mut pane = Pane::new("\t\t");
        pane.content.move_to(cursor_at(0, 2));
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "\t");
        // A selection is deleted, and nothing else.
        let mut pane = Pane::new("        x");
        select_from(&mut pane, (0, 6), (0, 7));
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
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
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.model.text(), "    one\ntwo");
        assert_eq!(pane.suggestions()[0].range, 8..11);
        // In spaces a suggestion changes, it is refused and the caret stays put.
        let mut pane = Pane::new("        x");
        pane.suggest(5..6, "").unwrap();
        pane.put_caret(8);
        let cursor = pane.content.cursor();
        assert_eq!(
            pane.backspace(4, Syntax::Plain),
            Err(IN_A_SUGGESTION.to_string())
        );
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
    fn alt_up_and_down_move_the_caret_line_and_the_caret_with_it() {
        let mut pane = Pane::new("one\ntwo\nthree");
        pane.content.move_to(cursor_at(2, 3));
        assert_eq!(pane.move_lines(true), Ok(true));
        assert_eq!(pane.content.text(), "one\nthree\ntwo");
        assert_eq!(pane.content.cursor(), cursor_at(1, 3));
        assert_eq!(pane.move_lines(true), Ok(true));
        assert_eq!(pane.content.text(), "three\none\ntwo");
        assert_eq!(pane.move_lines(true), Ok(false), "already at the top");
        assert_eq!(pane.content.cursor(), cursor_at(0, 3));
        // Down to the last line, which has no newline after it.
        assert_eq!(pane.move_lines(false), Ok(true));
        assert_eq!(pane.move_lines(false), Ok(true));
        assert_eq!(pane.content.text(), "one\ntwo\nthree");
        assert_eq!(pane.content.cursor(), cursor_at(2, 3));
        assert_eq!(pane.move_lines(false), Ok(false), "already at the bottom");
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(
            pane.content.text(),
            "one\nthree\ntwo",
            "each move is one undo step"
        );
    }

    #[test]
    fn alt_up_and_down_move_every_selected_line() {
        let mut pane = Pane::new("a\nbé\nc\nd\n");
        // From inside "bé" to the start of "d": nothing on "d" is selected, so it stays.
        let selected = |caret: (usize, usize), anchor: (usize, usize)| Cursor {
            position: Position {
                line: caret.0,
                column: caret.1,
            },
            selection: Some(Position {
                line: anchor.0,
                column: anchor.1,
            }),
        };
        pane.content.move_to(selected((3, 0), (1, 1)));
        assert_eq!(pane.move_lines(true), Ok(true));
        assert_eq!(pane.content.text(), "bé\nc\na\nd\n");
        assert_eq!(pane.content.cursor(), selected((2, 0), (0, 1)));
        assert_eq!(pane.move_lines(false), Ok(true));
        assert_eq!(pane.move_lines(false), Ok(true));
        assert_eq!(pane.content.text(), "a\nd\nbé\nc\n");
        assert_eq!(pane.content.cursor(), selected((4, 0), (2, 1)));
        assert_eq!(pane.copied().as_deref(), Some("é\nc\n"));
        pane.undo();
        pane.undo();
        pane.undo();
        assert_eq!(pane.content.text(), "a\nbé\nc\nd\n");
    }

    #[test]
    fn moving_lines_while_reviewing() {
        let mut pane = Pane::new("one\ntwo\nthree");
        pane.suggest(0..3, "1").unwrap();
        // "three" moves past "two", and the suggestion on "one" stays where it is.
        pane.put_caret(10);
        assert_eq!(pane.move_lines(true), Ok(true));
        assert_eq!(pane.model.text(), "one\nthree\ntwo");
        assert_eq!(pane.suggestions()[0].range, 0..3);
        assert_eq!(pane.caret(), 6);
        // It can't move past "one", which has the suggestion on it.
        assert_eq!(pane.move_lines(true), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.model.text(), "one\nthree\ntwo");
        assert!(pane.undo());
        assert_eq!(pane.model.text(), "one\ntwo\nthree");
        assert_eq!(pane.suggestions()[0].range, 0..3);
    }

    #[test]
    fn only_alt_with_an_arrow_moves_lines() {
        let moves = |key, modifiers| match binding_for(key, modifiers) {
            Some(Binding::Custom(Message::MoveLines { up })) => Some(up),
            _ => None,
        };
        assert_eq!(moves(Named::ArrowUp, Modifiers::ALT), Some(true));
        assert_eq!(moves(Named::ArrowDown, Modifiers::ALT), Some(false));
        assert_eq!(
            moves(Named::ArrowUp, Modifiers::ALT | Modifiers::SHIFT),
            None
        );
        assert_eq!(
            moves(Named::ArrowDown, Modifiers::ALT | Modifiers::COMMAND),
            None
        );
        assert_eq!(moves(Named::ArrowUp, Modifiers::empty()), None);
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
        // On a line of only spaces before the caret, the spaces a suggestion changes move down
        // with the rest, and the suggestion with them.
        let mut pane = Pane::new("    x");
        pane.suggest(0..2, "").unwrap();
        pane.put_caret(4);
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.model.text(), "\n    x");
        assert_eq!(pane.suggestions()[0].range, 1..3);
    }

    #[test]
    fn markdown_enter_continues_a_list_and_ends_it_on_an_empty_item() {
        let mut pane = Pane::new("- café");
        pane.content.move_to(cursor_at(0, 7));
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "- café\n- ");
        assert_eq!(pane.content.cursor(), cursor_at(1, 2));
        // Enter on the empty item takes its marker off instead of adding a line.
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "- café\n");
        assert_eq!(pane.content.cursor(), cursor_at(1, 0));
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(
            pane.content.text(),
            "- café\n- ",
            "each Enter is one undo step"
        );
        pane.undo();
        assert_eq!(pane.content.text(), "- café");
        // A selection is replaced, and the numbers after aren't changed.
        let mut pane = Pane::new("1. one two\n2. three");
        pane.select(6..10);
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "1. one\n2. \n2. three");
        assert_eq!(pane.content.cursor(), cursor_at(1, 3));
    }

    #[test]
    fn markdown_enter_on_an_empty_item_takes_off_one_layer() {
        // An indented item unindents, as Shift+Tab does.
        let mut pane = Pane::new("- a\n    - ");
        pane.content.move_to(cursor_at(1, 6));
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "- a\n- ");
        assert_eq!(pane.content.cursor(), cursor_at(1, 2));
        // In a quote: the next item, then its marker goes, then the quote.
        let mut pane = Pane::new("> - a");
        pane.content.move_to(cursor_at(0, 5));
        for after in ["> - a\n> - ", "> - a\n> ", "> - a\n"] {
            assert_eq!(pane.markdown_new_line(4), Ok(true));
            assert_eq!(pane.content.text(), after);
        }
        assert_eq!(pane.content.cursor(), cursor_at(1, 0));
        // With its spaces selected, an empty item gets a plain new line.
        let mut pane = Pane::new("-   ");
        pane.select(2..4);
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "- \n");
    }

    #[test]
    fn markdown_enter_is_plain_in_fenced_code_and_in_the_prefix() {
        let mut pane = Pane::new("```\n  - code");
        pane.content.move_to(cursor_at(1, 8));
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "```\n  - code\n  ");
        let mut pane = Pane::new("- item");
        pane.content.move_to(cursor_at(0, 1));
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "-\n item");
        let mut pane = Pane::new("");
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "\n");
    }

    #[test]
    fn markdown_enter_while_reviewing() {
        let mut pane = Pane::new("- one two");
        pane.suggest(6..9, "2").unwrap();
        pane.put_caret(5);
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.model.text(), "- one\n-  two");
        assert_eq!(pane.suggestions()[0].range, 9..12);
        // Taking off a marker a suggestion changes is refused, and the caret stays put.
        let mut pane = Pane::new("x\n- ");
        pane.suggest(2..3, "*").unwrap();
        pane.put_caret(4);
        let cursor = pane.content.cursor();
        assert_eq!(pane.markdown_new_line(4), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.content.cursor(), cursor);
        // So is unindenting an empty item with a suggestion on its line.
        let mut pane = Pane::new("  - \nx");
        pane.suggest(0..1, "").unwrap();
        pane.put_caret(4);
        assert_eq!(pane.markdown_new_line(4), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.model.text(), "  - \nx");
    }

    #[test]
    fn markdown_tab_on_a_list_item_nests_it() {
        let mut pane = Pane::new("- one\n- café");
        pane.content.move_to(cursor_at(1, 7));
        assert_eq!(pane.indent(4, true), Ok(true));
        assert_eq!(pane.content.text(), "- one\n    - café");
        assert_eq!(pane.content.cursor(), cursor_at(1, 11));
        // From the start of the line too, the caret stays on the same text.
        pane.content.move_to(cursor_at(0, 0));
        assert_eq!(pane.indent(2, true), Ok(true));
        assert_eq!(pane.content.text(), "  - one\n    - café");
        assert_eq!(pane.content.cursor(), cursor_at(0, 2));
        // So does a selection within the line.
        pane.select(14..19);
        assert_eq!(pane.indent(4, true), Ok(true));
        assert_eq!(pane.content.text(), "  - one\n        - café");
        assert_eq!(pane.copied().as_deref(), Some("café"));
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(pane.content.text(), "  - one\n    - café", "one undo step");
    }

    #[test]
    fn markdown_tab_elsewhere_types_spaces() {
        // Not a list item, in fenced code, or not Markdown: the spaces go at the caret.
        for (text, markdown) in [("one", true), ("```\n- x", true), ("- x", false)] {
            let mut pane = Pane::new(text);
            pane.content.move_to(Cursor {
                position: pane.position(text.len(), true),
                selection: None,
            });
            assert_eq!(pane.indent(2, markdown), Ok(true));
            assert_eq!(pane.content.text(), format!("{text}  "));
        }
    }

    #[test]
    fn markdown_tab_on_a_list_item_while_reviewing() {
        let mut pane = Pane::new("- one two\n- three");
        pane.suggest(6..9, "2").unwrap();
        // The item with the suggestion on it can't be nested.
        pane.put_caret(3);
        assert_eq!(pane.indent(4, true), Err(IN_A_SUGGESTION.to_string()));
        // The other can, and the suggestion stays where it is.
        pane.put_caret(14);
        assert_eq!(pane.indent(4, true), Ok(true));
        assert_eq!(pane.model.text(), "- one two\n    - three");
        assert_eq!(pane.suggestions()[0].range, 6..9);
        assert_eq!(pane.caret(), 18);
    }

    #[test]
    fn ctrl_shift_l_is_left_for_the_app() {
        // The app turns line numbers on and off with it, as Ctrl+L opens the assistant.
        let press = |modifiers| KeyPress {
            key: Key::Character("l".into()),
            modified_key: Key::Character("L".into()),
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        assert!(key_binding(press(Modifiers::COMMAND | Modifiers::SHIFT), true, false).is_none());
        assert!(key_binding(press(Modifiers::COMMAND), true, false).is_none());
    }

    #[test]
    fn ctrl_b_and_ctrl_i_toggle_bold_and_italic_markers() {
        let press = |key: &str| KeyPress {
            key: Key::Character(key.into()),
            modified_key: Key::Character(key.into()),
            physical_key: Physical::Unidentified(NativeCode::Unidentified),
            modifiers: Modifiers::COMMAND,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        let marker = |key| match key_binding(press(key), true, false) {
            Some(Binding::Custom(Message::ToggleMarker(marker))) => Some(marker),
            _ => None,
        };
        assert_eq!((marker("b"), marker("i")), (Some(BOLD), Some(ITALIC)));
        // A read-only pane takes neither.
        assert!(key_binding(press("b"), false, false).is_none());

        let mut pane = Pane::new("make café bold");
        pane.select(5..10);
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.content.text(), "make **café** bold");
        assert_eq!(pane.copied().as_deref(), Some("café"));
        // Again, with the markers just outside the selection, takes them off.
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.content.text(), "make café bold");
        assert_eq!(pane.copied().as_deref(), Some("café"));
        // So does a selection with the markers at its ends.
        assert_eq!(pane.toggle_marker(ITALIC), Ok(true));
        pane.select(5..12);
        assert_eq!(pane.toggle_marker(ITALIC), Ok(true));
        assert_eq!(pane.content.text(), "make café bold");
        assert_eq!(pane.copied().as_deref(), Some("café"));
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(
            pane.content.text(),
            "make _café_ bold",
            "each is one undo step"
        );
    }

    #[test]
    fn ctrl_b_with_nothing_selected_puts_the_caret_between_a_pair() {
        let mut pane = Pane::new("a\nb");
        pane.content.move_to(cursor_at(1, 1));
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.content.text(), "a\nb****");
        assert_eq!(pane.content.cursor(), cursor_at(1, 3));
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.content.text(), "a\nb");
        assert_eq!(pane.content.cursor(), cursor_at(1, 1));
        let mut pane = Pane::new("");
        assert_eq!(pane.toggle_marker(ITALIC), Ok(true));
        assert_eq!(pane.content.text(), "__");
        assert_eq!(pane.content.cursor(), cursor_at(0, 1));
    }

    #[test]
    fn markers_while_reviewing() {
        let mut pane = Pane::new("one two three");
        pane.suggest(4..7, "2").unwrap();
        pane.select(8..13);
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.model.text(), "one two **three**");
        assert_eq!(pane.suggestions()[0].range, 4..7);
        assert_eq!(pane.copied().as_deref(), Some("three"));
        // Around the text a suggestion changes, it is refused and the selection put back.
        pane.select(0..7);
        let cursor = pane.content.cursor();
        assert_eq!(pane.toggle_marker(ITALIC), Err(IN_A_SUGGESTION.to_string()));
        assert_eq!(pane.content.cursor(), cursor);
        assert_eq!(pane.model.text(), "one two **three**");
    }

    /// A double click (`Action::SelectWord`) or triple click (`Action::SelectLine`) at `line` and
    /// `column`, as the widget makes it: the caret goes there, then the word or line is selected.
    fn click(pane: &mut Pane, line: usize, column: usize, select: Action) {
        pane.content.move_to(cursor_at(line, column));
        pane.perform(select).unwrap();
    }

    #[test]
    fn a_double_or_triple_click_selects_the_whole_word_or_line() {
        let mut pane = Pane::new("make this bold");
        click(&mut pane, 0, 7, Action::SelectWord);
        assert_eq!(pane.selection(), Some(5..9));
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.content.text(), "make **this** bold");
        // At the end of the line, where iced's cursor is past the word's start.
        let mut pane = Pane::new("make this bold");
        click(&mut pane, 0, 14, Action::SelectWord);
        assert_eq!(pane.toggle_marker(BOLD), Ok(true));
        assert_eq!(pane.content.text(), "make this **bold**");
        let mut pane = Pane::new("one line\ntwo");
        click(&mut pane, 0, 2, Action::SelectLine);
        assert_eq!(pane.toggle_marker(ITALIC), Ok(true));
        assert_eq!(pane.content.text(), "_one line_\ntwo");
        // Grown by Shift+→ (or a drag) after the double click, it still reaches whole words.
        let mut pane = Pane::new("one two three");
        click(&mut pane, 0, 5, Action::SelectWord);
        for _ in 0..4 {
            pane.perform(Action::Select(Motion::Right)).unwrap();
        }
        assert_eq!(pane.copied().as_deref(), Some("two three"));
        // A click ends it.
        pane.perform(Action::Move(Motion::Left)).unwrap();
        assert_eq!(pane.selection(), None);
    }

    #[test]
    fn backspace_and_enter_replace_a_double_or_triple_click_selection() {
        let mut pane = Pane::new("a\n    indented line\nb");
        click(&mut pane, 1, 2, Action::SelectLine);
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "a\n\nb");
        assert_eq!(pane.model.text(), pane.content.text());
        let mut pane = Pane::new("a\n    indented line\nb");
        click(&mut pane, 1, 2, Action::SelectLine);
        assert_eq!(pane.new_line(), Ok(true));
        assert_eq!(pane.content.text(), "a\n\n\nb");
        let mut pane = Pane::new("- item one");
        click(&mut pane, 0, 8, Action::SelectWord);
        assert_eq!(pane.markdown_new_line(4), Ok(true));
        assert_eq!(pane.content.text(), "- item \n- ");
        assert_eq!(pane.model.text(), pane.content.text());
    }

    #[test]
    fn copying_a_double_clicked_word_while_reviewing() {
        let (mut pane, _, _) = reviewing_pane();
        let shown = pane.content.text();
        let stay = shown.find("Stay").unwrap();
        click(&mut pane, 0, stay + 1, Action::SelectWord);
        assert_eq!(pane.copied().as_deref(), Some("Stay"));
    }

    #[test]
    fn replace_all_is_one_undo_step_and_spares_suggestions() {
        let mut pane = Pane::new("cat and cat and cat");
        assert_eq!(pane.replace_all(&[0..3, 8..11], "dog"), Ok(true));
        assert_eq!(pane.content.text(), "dog and dog and cat");
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "cat and cat and cat");
        pane.suggest(16..19, "cow").unwrap();
        assert_eq!(
            pane.replace_all(&[0..3, 16..19], "dog"),
            Err(IN_A_SUGGESTION.to_string())
        );
        assert_eq!(pane.model.text(), "cat and cat and cat");
    }

    #[test]
    fn go_to_line_puts_the_caret_at_its_start_or_the_end_of_the_text() {
        let mut pane = Pane::new("one\ntwo\nthree");
        pane.go_to_line(1);
        assert_eq!(pane.caret(), 4);
        pane.go_to_line(9);
        assert_eq!(pane.caret(), 13);
    }

    #[test]
    fn duplicating_lines_moves_the_selection_onto_the_copy() {
        let mut pane = Pane::new("one\ntwo\nthree");
        select_from(&mut pane, (0, 1), (1, 2));
        assert_eq!(pane.duplicate_lines(), Ok(true));
        assert_eq!(pane.content.text(), "one\ntwo\none\ntwo\nthree");
        assert_eq!(pane.selection(), Some(9..14), "\"ne\\ntw\" in the copy");
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "one\ntwo\nthree");
    }

    #[test]
    fn deleting_lines_keeps_the_caret_column_where_it_can() {
        let mut pane = Pane::new("first line\nsecond line\nend");
        pane.content.move_to(cursor_at(0, 7));
        assert_eq!(pane.delete_lines(), Ok(true));
        assert_eq!(pane.content.text(), "second line\nend");
        assert_eq!(pane.caret(), 7);
        pane.content.move_to(cursor_at(1, 2));
        pane.delete_lines().unwrap();
        assert_eq!(pane.content.text(), "second line");
        assert_eq!(
            pane.caret(),
            2,
            "the line above came up, so the caret is on it"
        );
        pane.suggest(0..6, "first").unwrap();
        assert_eq!(pane.delete_lines(), Err(IN_A_SUGGESTION.to_string()));
    }

    #[test]
    fn ctrl_d_selects_the_word_then_each_next_match_round_to_the_top() {
        let mut pane = Pane::new("cat dog cat Cat catalog cat");
        pane.content.move_to(cursor_at(0, 9));
        assert_eq!(pane.select_next_match(), Ok(true));
        assert_eq!(pane.selection(), Some(8..11), "the word at the caret");
        assert_eq!(pane.select_next_match(), Ok(true));
        assert_eq!(
            pane.selection(),
            Some(24..27),
            "case matters, and whole words"
        );
        assert_eq!(pane.select_next_match(), Ok(true));
        assert_eq!(pane.selection(), Some(0..3), "round to the top");
        assert_eq!(pane.select_next_match(), Ok(false), "every one is selected");
        assert_eq!(pane.others, vec![8..11, 24..27]);
        // Part of a word matches inside words too.
        pane.select(17..19);
        pane.select_next_match().unwrap();
        assert_eq!(pane.selection(), Some(25..27));
    }

    #[test]
    fn typing_after_ctrl_d_changes_every_place_as_one_step_each() {
        let mut pane = Pane::new("cat dog cat");
        pane.content.move_to(cursor_at(0, 1));
        pane.select_next_match().unwrap();
        pane.select_next_match().unwrap();
        for c in "cow".chars() {
            assert_eq!(pane.perform(Action::Edit(Edit::Insert(c))), Ok(true));
        }
        assert_eq!(pane.content.text(), "cow dog cow");
        assert_eq!(pane.model.text(), "cow dog cow");
        assert_eq!(
            pane.caret(),
            11,
            "the widget's own caret, after the last cow"
        );
        assert_eq!(pane.others, vec![3..3]);
        assert_eq!(pane.backspace(4, Syntax::Plain), Ok(true));
        assert_eq!(pane.content.text(), "co dog co");
        assert!(pane.undo());
        assert_eq!(pane.content.text(), "cow dog cow");
        assert!(pane.others.is_empty(), "undo lets the other places go");
    }

    #[test]
    fn moving_the_caret_lets_ctrl_d_places_go() {
        let mut pane = Pane::new("ab ab");
        pane.content.move_to(cursor_at(0, 0));
        pane.select_next_match().unwrap();
        pane.select_next_match().unwrap();
        pane.perform(Action::Move(Motion::Right)).unwrap();
        pane.perform(Action::Edit(Edit::Insert('x'))).unwrap();
        assert_eq!(pane.content.text(), "ab abx");
    }

    #[test]
    fn deleting_at_carets_side_by_side_takes_each_character_once() {
        let mut pane = Pane::new("aaa");
        pane.content.move_to(cursor_at(0, 0));
        pane.select(0..1);
        pane.others = vec![1..2, 2..3];
        pane.perform(Action::Edit(Edit::Delete)).unwrap();
        assert_eq!(pane.content.text(), "");
        let mut pane = Pane::new("abcd");
        pane.put_caret(2);
        pane.others.push(1..1);
        pane.perform(Action::Edit(Edit::Backspace)).unwrap();
        assert_eq!(
            pane.content.text(),
            "cd",
            "the a before one caret, the b before the other"
        );
        assert_eq!(pane.caret(), 0);
        assert!(pane.others.is_empty(), "the two carets met, so they're one");
    }

    #[test]
    fn typing_a_block_in_code_closes_pairs_and_indents() {
        let shell = Syntax::Shell;
        let mut pane = Pane::new("main() ");
        pane.content.move_to(cursor_at(0, 7));
        assert_eq!(pane.type_in_code('{', shell), Ok(true));
        assert_eq!(pane.content.text(), "main() {}");
        assert_eq!(pane.content.cursor(), cursor_at(0, 8));
        assert_eq!(pane.code_new_line(4, shell), Ok(true));
        assert_eq!(pane.content.text(), "main() {\n    \n}");
        assert_eq!(pane.content.cursor(), cursor_at(1, 4));
        assert_eq!(pane.type_in_code('"', shell), Ok(true));
        assert_eq!(pane.content.text(), "main() {\n    \"\"\n}");
        // Backspace between the quotes takes both, and is one undo step.
        assert_eq!(pane.backspace(4, shell), Ok(true));
        assert_eq!(pane.content.text(), "main() {\n    \n}");
        assert_eq!(pane.model.text(), pane.content.text());
        pane.undo();
        assert_eq!(pane.content.text(), "main() {\n    \"\"\n}");
        // Typing the closer steps over it.
        pane.content.move_to(cursor_at(1, 5));
        assert_eq!(pane.type_in_code('"', shell), Ok(false));
        assert_eq!(pane.content.cursor(), cursor_at(1, 6));
        // A selection gets the pair around it and stays selected.
        let mut pane = Pane::new("echo $x");
        pane.select(5..7);
        assert_eq!(pane.type_in_code('"', shell), Ok(true));
        assert_eq!(pane.content.text(), "echo \"$x\"");
        assert_eq!(pane.selection(), Some(6..8));
        // Other characters are typed as usual.
        assert_eq!(pane.type_in_code('a', shell), Ok(true));
        assert_eq!(pane.content.text(), "echo \"a\"");
    }

    #[test]
    fn ctrl_slash_comments_the_selected_lines_and_keeps_them_selected() {
        let mut pane = Pane::new("a=1\n  b=2\nc=3");
        pane.select(2..7);
        assert_eq!(pane.toggle_comment("#"), Ok(true));
        assert_eq!(pane.content.text(), "# a=1\n#   b=2\nc=3");
        assert_eq!(pane.selection(), Some(4..11), "still from `=1` to `b=`");
        assert_eq!(pane.toggle_comment("#"), Ok(true));
        assert_eq!(pane.content.text(), "a=1\n  b=2\nc=3");
        assert_eq!(pane.selection(), Some(2..7));
        pane.undo();
        assert_eq!(pane.content.text(), "# a=1\n#   b=2\nc=3", "one undo step");
        // With only the caret, its line.
        let mut pane = Pane::new("x\ny");
        pane.content.move_to(cursor_at(1, 1));
        assert_eq!(pane.toggle_comment("#"), Ok(true));
        assert_eq!(pane.content.text(), "x\n# y");
        assert_eq!(pane.content.cursor(), cursor_at(1, 3));
    }

    #[test]
    fn ctrl_shift_x_ticks_the_selected_lines_tasks_and_keeps_them_selected() {
        let mut pane = Pane::new("- [ ] a\n- [ ] b\nc");
        pane.select(6..17);
        assert_eq!(pane.toggle_tasks(), Ok(true));
        assert_eq!(
            pane.content.text(),
            "- [ ] a\n- [ ] b\n- [ ] c",
            "c gets a box first"
        );
        assert_eq!(pane.selection(), Some(6..23), "still from `a` to `c`");
        assert_eq!(pane.toggle_tasks(), Ok(true));
        assert_eq!(pane.content.text(), "- [x] a\n- [x] b\n- [x] c");
        assert_eq!(pane.selection(), Some(6..23));
        pane.undo();
        assert_eq!(
            pane.content.text(),
            "- [ ] a\n- [ ] b\n- [ ] c",
            "one undo step"
        );
        // With only the caret, its line; the caret stays on its text.
        let mut pane = Pane::new("- [ ] a");
        pane.content.move_to(cursor_at(0, 7));
        assert_eq!(pane.toggle_tasks(), Ok(true));
        assert_eq!(pane.content.text(), "- [x] a");
        assert_eq!(pane.content.cursor(), cursor_at(0, 7));
    }
}
