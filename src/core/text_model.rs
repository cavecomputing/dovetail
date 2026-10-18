//! The text behind an editor pane, as plain Rust: a copy of the text, the undo and redo history,
//! and the paint (which stretches belong to which idea).
//!
//! The editor widget stays in charge of what is on screen. After each edit the UI hands the new
//! text here; `record` works out the one `Change` that was made and keeps it for undo. Changes
//! made here first (undo, redo, unindent, `replace`) are returned so the UI can replay them into
//! the widget. All offsets are bytes, the same as iced's columns.

use std::ops::{Range, RangeInclusive};
use std::time::{Duration, Instant};

use crate::core::review::Suggestion;
use crate::project::paint::{self, IdeaId, Span};

/// How many undo steps are kept; the oldest is dropped beyond this.
const MAX_UNDO: usize = 1000;
/// Typing within this long of the last keystroke joins the same undo step.
const TYPING_PAUSE: Duration = Duration::from_secs(1);

/// One edit: at byte `at`, `removed` was replaced by `inserted`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl Change {
    /// The change that undoes this one.
    fn inverse(&self) -> Change {
        Change {
            at: self.at,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }

    /// One typed character (not Enter), or one character deleted by Backspace or Delete.
    fn is_typing(&self) -> bool {
        let single = |text: &str| text.chars().count() == 1 && text != "\n";
        (self.removed.is_empty() && single(&self.inserted))
            || (self.inserted.is_empty() && single(&self.removed))
    }
}

/// What one Ctrl+Z undoes.
struct Step {
    changes: Vec<Change>,
    /// Whether more typing may still join this step.
    typing: bool,
    at: Instant,
    paint_before: Vec<Span>,
    paint_after: Vec<Span>,
}

pub struct TextModel {
    /// The same text the widget shows, with `\n` line endings.
    text: String,
    /// The paint on this text, kept in step with every change.
    pub paint: Vec<Span>,
    /// When set, newly typed or pasted text is painted with this idea.
    pub brush: Option<IdeaId>,
    /// The text last cut or copied (from either pane) and its paint, from `clipped`. Pasting
    /// exactly that text puts the paint back.
    pub clip: Option<(String, Vec<Span>)>,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl TextModel {
    pub fn new(text: String) -> TextModel {
        TextModel {
            text,
            paint: Vec::new(),
            brush: None,
            clip: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text in `range` and the paint on it, moved to start at 0: what Cut and Copy keep for
    /// `clip`.
    pub fn clipped(&self, range: Range<usize>) -> (String, Vec<Span>) {
        let paint = self
            .paint
            .iter()
            .filter_map(|span| {
                let start = span.start.max(range.start);
                let end = span.end.min(range.end);
                (start < end).then(|| Span {
                    start: start - range.start,
                    end: end - range.start,
                    ..span.clone()
                })
            })
            .collect();
        (self.text[range].to_string(), paint)
    }

    /// The single change that turns `old` into `new`, or `None` if they're equal. `edit_start`
    /// is where the edit began in `old` (the start of the selection, or the character before
    /// the cursor for Backspace); the change never starts after it, so typing "a" into "aa"
    /// lands where the cursor was.
    pub fn change_between(old: &str, new: &str, edit_start: usize) -> Option<Change> {
        if old == new {
            return None;
        }
        let same_start = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let mut prefix = same_start.min(edit_start);
        while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let same_end = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let mut suffix = same_end.min(old.len().min(new.len()) - prefix);
        while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix)
        {
            suffix -= 1;
        }
        Some(Change {
            at: prefix,
            removed: old[prefix..old.len() - suffix].to_string(),
            inserted: new[prefix..new.len() - suffix].to_string(),
        })
    }

    /// Records an edit the widget made: `new_text` is its text now, `edit_start` as in
    /// `change_between`. Returns the change, or `None` if the text didn't change.
    pub fn record(&mut self, new_text: String, edit_start: usize, now: Instant) -> Option<Change> {
        let change = TextModel::change_between(&self.text, &new_text, edit_start)?;
        let paint_before = self.paint.clone();
        self.text = new_text;
        paint::shift(&mut self.paint, &change);
        paint::continue_word(&mut self.paint, &change, &self.text);
        // Pasting what was cut or copied puts its paint back; otherwise the brush paints it.
        let clip = self
            .clip
            .as_ref()
            .filter(|(clipped, spans)| *clipped == change.inserted && !spans.is_empty());
        if let Some((_, spans)) = clip {
            for span in spans {
                let range = change.at + span.start..change.at + span.end;
                paint::brush(&mut self.paint, range, span.idea);
            }
        } else if let Some(idea) = self.brush {
            paint::brush(
                &mut self.paint,
                change.at..change.at + change.inserted.len(),
                idea,
            );
        }
        self.redo.clear();
        let typing = change.is_typing();
        if let Some(last) = self.undo.last_mut()
            && typing
            && last.typing
            && now.duration_since(last.at) < TYPING_PAUSE
            && join_typing(&mut last.changes[0], &change)
        {
            last.at = now;
            last.paint_after = self.paint.clone();
            return Some(change);
        }
        self.push_step(vec![change.clone()], typing, now, paint_before);
        Some(change)
    }

    /// Replaces `range` with `text` as its own undo step, and returns the change to replay.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Change {
        let change = Change {
            at: range.start,
            removed: self.text[range].to_string(),
            inserted: text.to_string(),
        };
        let paint_before = self.paint.clone();
        self.apply(&change);
        self.redo.clear();
        self.push_step(vec![change.clone()], false, Instant::now(), paint_before);
        change
    }

    /// Accepts `suggestions` (in text order, not overlapping) as one undo step: each puts its
    /// text in place of its range, and text whose suggestion names an idea takes that idea's
    /// paint. They are applied from the last back, so each range is still right when its turn
    /// comes. Returns the changes to replay, in order.
    pub fn accept(&mut self, suggestions: &[Suggestion]) -> Vec<Change> {
        let paint_before = self.paint.clone();
        let mut changes = Vec::new();
        for suggestion in suggestions.iter().rev() {
            let change = Change {
                at: suggestion.range.start,
                removed: self.text[suggestion.range.clone()].to_string(),
                inserted: suggestion.text.clone(),
            };
            self.apply(&change);
            if let Some(idea) = suggestion.idea {
                let end = change.at + change.inserted.len();
                paint::brush(&mut self.paint, change.at..end, idea);
            }
            changes.push(change);
        }
        if !changes.is_empty() {
            self.redo.clear();
            self.push_step(changes.clone(), false, Instant::now(), paint_before);
        }
        changes
    }

    /// Shift+Tab: removes up to `tab_spaces` leading spaces from each of `lines`, as one undo
    /// step. Returns one change per line that had any, top line first, so paint inside the lines
    /// stays put.
    pub fn unindent(&mut self, lines: RangeInclusive<usize>, tab_spaces: usize) -> Vec<Change> {
        let paint_before = self.paint.clone();
        let mut changes = Vec::new();
        for line in lines {
            let at = offset(&self.text, line, 0);
            let spaces = self.text[at..]
                .bytes()
                .take(tab_spaces)
                .take_while(|byte| *byte == b' ')
                .count();
            if spaces > 0 {
                let change = Change {
                    at,
                    removed: " ".repeat(spaces),
                    inserted: String::new(),
                };
                self.apply(&change);
                changes.push(change);
            }
        }
        if !changes.is_empty() {
            self.redo.clear();
            self.push_step(changes.clone(), false, Instant::now(), paint_before);
        }
        changes
    }

    /// Replaces the paint (painting or erasing a selection, making an idea) as its own undo step.
    pub fn set_paint(&mut self, paint: Vec<Span>) {
        let paint_before = std::mem::replace(&mut self.paint, paint);
        self.redo.clear();
        self.push_step(Vec::new(), false, Instant::now(), paint_before);
    }

    /// Removes every trace of `idea`'s paint, from the undo and redo history too, so undo can't
    /// bring back paint of a deleted idea.
    pub fn forget_idea(&mut self, idea: IdeaId) {
        self.paint.retain(|span| span.idea != idea);
        for step in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            step.paint_before.retain(|span| span.idea != idea);
            step.paint_after.retain(|span| span.idea != idea);
        }
    }

    /// Undoes the last step. Returns the changes to replay into the widget, in order (empty
    /// when there was nothing to undo, or the step only changed paint).
    pub fn undo(&mut self) -> Vec<Change> {
        let Some(mut step) = self.undo.pop() else {
            return Vec::new();
        };
        let changes: Vec<Change> = step.changes.iter().rev().map(Change::inverse).collect();
        for change in &changes {
            self.text.replace_range(
                change.at..change.at + change.removed.len(),
                &change.inserted,
            );
        }
        self.paint = step.paint_before.clone();
        step.typing = false;
        self.redo.push(step);
        if let Some(last) = self.undo.last_mut() {
            last.typing = false;
        }
        changes
    }

    /// Redoes the last undone step; returns the changes to replay, like `undo`.
    pub fn redo(&mut self) -> Vec<Change> {
        let Some(step) = self.redo.pop() else {
            return Vec::new();
        };
        for change in &step.changes {
            self.text.replace_range(
                change.at..change.at + change.removed.len(),
                &change.inserted,
            );
        }
        self.paint = step.paint_after.clone();
        let changes = step.changes.clone();
        self.undo.push(step);
        changes
    }

    fn apply(&mut self, change: &Change) {
        self.text.replace_range(
            change.at..change.at + change.removed.len(),
            &change.inserted,
        );
        paint::shift(&mut self.paint, change);
    }

    fn push_step(
        &mut self,
        changes: Vec<Change>,
        typing: bool,
        at: Instant,
        paint_before: Vec<Span>,
    ) {
        self.undo.push(Step {
            changes,
            typing,
            at,
            paint_before,
            paint_after: self.paint.clone(),
        });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }
}

/// The byte offset in `text` of (line, byte column), clamped to the text.
pub fn offset(text: &str, line: usize, column: usize) -> usize {
    let line_start: usize = text.split('\n').take(line).map(|l| l.len() + 1).sum();
    (line_start + column).min(text.len())
}

/// The (line, byte column) of a byte offset in `text`.
pub fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let column = offset - before.rfind('\n').map_or(0, |at| at + 1);
    (line, column)
}

/// The (line, byte column) of each of `offsets` (in order) in `text`, in one pass over the text
/// rather than one per offset.
pub fn positions(text: &str, offsets: &[usize]) -> Vec<(usize, usize)> {
    let (mut line, mut line_start, mut scanned) = (0, 0, 0);
    let mut found = Vec::with_capacity(offsets.len());
    for &offset in offsets {
        for (at, _) in text[scanned..offset].match_indices('\n') {
            line += 1;
            line_start = scanned + at + 1;
        }
        scanned = offset;
        found.push((line, offset - line_start));
    }
    found
}

/// Folds one more typed or deleted character into the step's change, if it carries straight on
/// from it. Typing stops joining at the start of a new word.
fn join_typing(last: &mut Change, next: &Change) -> bool {
    if last.removed.is_empty() && next.removed.is_empty() {
        let starts_word = last.inserted.ends_with(char::is_whitespace)
            && !next.inserted.starts_with(char::is_whitespace);
        if next.at == last.at + last.inserted.len() && !starts_word {
            last.inserted.push_str(&next.inserted);
            return true;
        }
    } else if last.inserted.is_empty() && next.inserted.is_empty() {
        // Backspace runs leftwards, Delete stays in place.
        if next.at + next.removed.len() == last.at {
            last.at = next.at;
            last.removed.insert_str(0, &next.removed);
            return true;
        }
        if next.at == last.at {
            last.removed.push_str(&next.removed);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_match_position_for_each_offset() {
        let text = "ab\ncdé\n\nf";
        let offsets = [0, 1, 3, 5, 7, 8, 9, 10];
        let one_by_one: Vec<_> = offsets.iter().map(|&o| position(text, o)).collect();
        assert_eq!(positions(text, &offsets), one_by_one);
    }

    fn change(at: usize, removed: &str, inserted: &str) -> Option<Change> {
        Some(Change {
            at,
            removed: removed.to_string(),
            inserted: inserted.to_string(),
        })
    }

    /// Types `text` one character at a time from byte `at`, all at the same moment.
    fn type_at(model: &mut TextModel, mut at: usize, text: &str, now: Instant) {
        for c in text.chars() {
            let mut new_text = model.text().to_string();
            new_text.insert(at, c);
            model.record(new_text, at, now);
            at += c.len_utf8();
        }
    }

    fn backspace(model: &mut TextModel, at: usize, now: Instant) {
        let mut new_text = model.text().to_string();
        new_text.remove(at - 1);
        model.record(new_text, at - 1, now);
    }

    #[test]
    fn the_change_starts_no_later_than_the_edit() {
        // Typing "a" into "aa" at the cursor, at the start, and at the end.
        assert_eq!(
            TextModel::change_between("aa", "aaa", 1),
            change(1, "", "a")
        );
        assert_eq!(
            TextModel::change_between("aa", "aaa", 0),
            change(0, "", "a")
        );
        assert_eq!(
            TextModel::change_between("aa", "aaa", 2),
            change(2, "", "a")
        );
        // Backspace over the second "b" of "abb": the edit starts at the character before the
        // cursor, so it's that "b" that goes, not the last one.
        assert_eq!(
            TextModel::change_between("abb", "ab", 1),
            change(1, "b", "")
        );
        // Forward Delete at the cursor.
        assert_eq!(
            TextModel::change_between("abb", "ab", 2),
            change(2, "b", "")
        );
        // Pasting over a selection that starts at 2.
        assert_eq!(
            TextModel::change_between("one two six", "one tw two six", 4),
            change(4, "", "tw ")
        );
        assert_eq!(
            TextModel::change_between("the quick fox", "the slow fox", 4),
            change(4, "quick", "slow")
        );
        assert_eq!(TextModel::change_between("same", "same", 0), None);
    }

    #[test]
    fn the_change_never_splits_a_character() {
        // "é" and "è" share their first byte.
        assert_eq!(
            TextModel::change_between("café", "cafè", 3),
            change(3, "é", "è")
        );
        assert_eq!(TextModel::change_between("……", "…", 3), change(3, "…", ""));
    }

    #[test]
    fn a_word_is_one_undo_step_and_the_next_word_another() {
        let now = Instant::now();
        let mut model = TextModel::new(String::new());
        type_at(&mut model, 0, "hello world", now);
        assert_eq!(model.text(), "hello world");
        assert_eq!(
            model.undo(),
            vec![Change {
                at: 6,
                removed: "world".into(),
                inserted: "".into()
            }]
        );
        assert_eq!(model.text(), "hello ");
        model.undo();
        assert_eq!(model.text(), "");
    }

    #[test]
    fn enter_a_pause_or_a_jump_starts_a_new_step() {
        let now = Instant::now();
        let mut model = TextModel::new(String::new());
        type_at(&mut model, 0, "ab\ncd", now);
        model.undo();
        assert_eq!(model.text(), "ab\n");
        model.undo();
        assert_eq!(model.text(), "ab");

        type_at(&mut model, 2, "x", now + Duration::from_secs(5));
        assert_eq!(model.text(), "abx");
        model.undo();
        assert_eq!(model.text(), "ab");

        let mut model = TextModel::new("abc".to_string());
        type_at(&mut model, 3, "x", now);
        type_at(&mut model, 0, "y", now);
        model.undo();
        assert_eq!(model.text(), "abcx");
    }

    #[test]
    fn a_run_of_backspaces_is_one_step() {
        let now = Instant::now();
        let mut model = TextModel::new("hello".to_string());
        for at in (3..=5).rev() {
            backspace(&mut model, at, now);
        }
        assert_eq!(model.text(), "he");
        assert_eq!(
            model.undo(),
            vec![Change {
                at: 2,
                removed: "".into(),
                inserted: "llo".into()
            }]
        );
        assert_eq!(model.text(), "hello");
    }

    #[test]
    fn undo_all_restores_the_original_and_redo_brings_it_back() {
        let now = Instant::now();
        let mut model = TextModel::new("start".to_string());
        type_at(&mut model, 5, " one", now);
        model.replace(0..5, "begin");
        backspace(&mut model, 3, now);
        let edited = model.text().to_string();
        while !model.undo().is_empty() {}
        assert_eq!(model.text(), "start");
        while !model.redo().is_empty() {}
        assert_eq!(model.text(), edited);
    }

    #[test]
    fn undone_changes_replay_to_the_same_text() {
        // The UI copies returned changes into the widget one by one; doing the same to a plain
        // String must give the model's text.
        let now = Instant::now();
        let mut model = TextModel::new("    a\n    b\nc".to_string());
        model.unindent(0..=2, 4);
        type_at(&mut model, 0, "x", now);
        let mut widget = model.text().to_string();
        for _ in 0..2 {
            for change in model.undo() {
                widget.replace_range(
                    change.at..change.at + change.removed.len(),
                    &change.inserted,
                );
            }
            assert_eq!(widget, model.text());
        }
        for change in model.redo() {
            widget.replace_range(
                change.at..change.at + change.removed.len(),
                &change.inserted,
            );
        }
        assert_eq!(widget, model.text());
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let now = Instant::now();
        let mut model = TextModel::new(String::new());
        type_at(&mut model, 0, "one", now);
        model.undo();
        type_at(&mut model, 0, "two", now);
        assert!(model.redo().is_empty());
        assert_eq!(model.text(), "two");
    }

    #[test]
    fn typing_after_undo_does_not_join_an_older_step() {
        let now = Instant::now();
        let mut model = TextModel::new(String::new());
        type_at(&mut model, 0, "ab", now);
        type_at(&mut model, 2, " cd", now);
        model.undo();
        type_at(&mut model, 3, "x", now);
        assert_eq!(model.text(), "ab x");
        model.undo();
        assert_eq!(model.text(), "ab ");
    }

    #[test]
    fn only_the_last_thousand_steps_are_kept() {
        let mut model = TextModel::new(String::new());
        for n in 0..MAX_UNDO + 5 {
            let end = model.text().len();
            model.replace(end..end, &format!("{n},"));
        }
        let mut undone = 0;
        while !model.undo().is_empty() {
            undone += 1;
        }
        assert_eq!(undone, MAX_UNDO);
        assert_eq!(model.text(), "0,1,2,3,4,");
    }

    #[test]
    fn unindent_removes_up_to_one_indent_per_line() {
        let mut model = TextModel::new("        two levels\n  x\nnone\n    four".to_string());
        let changes = model.unindent(0..=3, 4);
        assert_eq!(model.text(), "    two levels\nx\nnone\nfour");
        assert_eq!(changes.len(), 3);
        assert_eq!(
            changes[1],
            Change {
                at: 15,
                removed: "  ".into(),
                inserted: "".into()
            }
        );
        model.undo();
        assert_eq!(model.text(), "        two levels\n  x\nnone\n    four");
        assert!(
            model.unindent(2..=2, 4).is_empty(),
            "nothing to remove is no step"
        );
    }

    #[test]
    fn offsets_and_positions_convert_both_ways() {
        let text = "ab\ncafé\n";
        assert_eq!(offset(text, 1, 5), 8);
        assert_eq!(position(text, 8), (1, 5));
        assert_eq!(offset(text, 2, 0), 9);
        assert_eq!(position(text, 9), (2, 0));
        assert_eq!(offset(text, 9, 9), 9, "past the end is clamped");
    }

    #[test]
    fn undo_restores_paint_a_delete_removed() {
        let mut model = TextModel::new("keep this part".to_string());
        let mut paint = Vec::new();
        paint::brush(&mut paint, 5..9, 1);
        model.set_paint(paint);
        model.replace(5..10, "");
        assert!(model.paint.is_empty(), "the painted word was deleted");
        model.undo();
        assert_eq!(model.text(), "keep this part");
        assert_eq!((model.paint[0].start, model.paint[0].end), (5, 9));
        // Painting is a step of its own.
        model.undo();
        assert!(model.paint.is_empty());
        model.redo();
        model.redo();
        assert!(model.paint.is_empty());
    }

    #[test]
    fn cut_and_paste_keeps_the_paint_even_with_another_brush() {
        let now = Instant::now();
        let mut model = TextModel::new("Nyx is quiet. She keeps her distance.".to_string());
        let mut paint = Vec::new();
        paint::brush(&mut paint, 14..37, 1);
        model.set_paint(paint);
        // Ctrl+X on " She keeps her distance." (13..37), then Ctrl+V at the start.
        model.clip = Some(model.clipped(13..37));
        model.record("Nyx is quiet.".to_string(), 13, now);
        assert!(model.paint.is_empty());
        model.brush = Some(2);
        model.record(" She keeps her distance.Nyx is quiet.".to_string(), 0, now);
        let painted: Vec<(IdeaId, &str)> = model
            .paint
            .iter()
            .map(|span| (span.idea, &model.text()[span.start..span.end]))
            .collect();
        assert_eq!(painted, [(1, "She keeps her distance.")]);
        // Other text still takes the brush.
        type_at(&mut model, 0, "x", now);
        assert!(model.paint.iter().any(|span| span.idea == 2));
    }

    #[test]
    fn retyping_the_last_painted_word_keeps_it_painted() {
        let now = Instant::now();
        let mut model = TextModel::new("calm but bold".to_string());
        model.set_paint(vec![Span {
            idea: 1,
            start: 4,
            end: 13,
            quote: String::new(),
        }]);
        // "bold" is selected and the first key replaces it, then the rest are typed.
        model.record("calm but b".to_string(), 9, now);
        type_at(&mut model, 10, "rave", now);
        assert_eq!(model.text(), "calm but brave");
        assert_eq!((model.paint[0].start, model.paint[0].end), (4, 14));
    }

    #[test]
    fn accepting_suggestions_is_one_undo_step_and_paints_with_their_idea() {
        let mut model = TextModel::new("Be quick. Stay calm.".to_string());
        let suggestion = |range, text: &str, idea| Suggestion {
            id: 0,
            range,
            text: text.to_string(),
            idea,
        };
        let changes = model.accept(&[
            suggestion(0..9, "Take your time.", Some(4)),
            suggestion(15..19, "steady", None),
        ]);
        assert_eq!(model.text(), "Take your time. Stay steady.");
        let painted: Vec<(IdeaId, &str)> = model
            .paint
            .iter()
            .map(|span| (span.idea, &model.text()[span.start..span.end]))
            .collect();
        assert_eq!(painted, [(4, "Take your time.")]);
        // The changes replay in order onto the old text.
        let mut widget = "Be quick. Stay calm.".to_string();
        for change in &changes {
            widget.replace_range(
                change.at..change.at + change.removed.len(),
                &change.inserted,
            );
        }
        assert_eq!(widget, model.text());
        model.undo();
        assert_eq!(model.text(), "Be quick. Stay calm.");
        assert!(model.paint.is_empty());
        model.redo();
        assert_eq!(model.text(), "Take your time. Stay steady.");
        assert_eq!(model.paint.len(), 1, "redo brings the paint back too");
    }

    #[test]
    fn typing_shifts_paint_and_the_brush_paints_new_text() {
        let now = Instant::now();
        let mut model = TextModel::new("ab cd".to_string());
        let mut paint = Vec::new();
        paint::brush(&mut paint, 3..5, 7);
        model.set_paint(paint);
        type_at(&mut model, 0, "zz", now);
        assert_eq!((model.paint[0].start, model.paint[0].end), (5, 7));
        model.brush = Some(7);
        type_at(&mut model, 7, "ef", now);
        assert_eq!(model.paint.len(), 1, "joined to the touching span");
        assert_eq!((model.paint[0].start, model.paint[0].end), (5, 9));
    }
}
