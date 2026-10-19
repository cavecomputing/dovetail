//! The text behind an editor pane, as plain Rust: a copy of the text and the undo and redo
//! history.
//!
//! The editor widget stays in charge of what is on screen. After each edit the UI hands the new
//! text here; `record` works out the one `Change` that was made and keeps it for undo. Changes
//! made here first (undo, redo, indent, unindent, moving lines, `replace`) are returned so the
//! UI can replay them into the widget. All offsets are bytes, the same as iced's columns.

use std::ops::{Range, RangeInclusive};
use std::time::{Duration, Instant};

use crate::core::review::Suggestion;

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
    /// Where the text it takes out is, before the change.
    pub fn removed_range(&self) -> Range<usize> {
        self.at..self.at + self.removed.len()
    }

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
}

pub struct TextModel {
    /// The same text the widget shows, with `\n` line endings.
    text: String,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl TextModel {
    pub fn new(text: String) -> TextModel {
        TextModel {
            text,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The single change that turns `old` into `new`, or `None` if they're equal. `edit_start`
    /// is where the edit began in `old` (the start of the selection, or the character before
    /// the cursor for Backspace); the change never starts after it, so typing "a" into "aa"
    /// lands where the cursor was.
    pub fn change_between(old: &str, new: &str, edit_start: usize) -> Option<Change> {
        if old == new {
            return None;
        }
        // This runs on every key, so the usual case (all before `edit_start` the same, and all
        // after the edit) is checked first with one whole-slice comparison each, which is many
        // times quicker on a long text than walking it byte by byte.
        let (old_bytes, new_bytes) = (old.as_bytes(), new.as_bytes());
        let mut prefix = match old_bytes.get(..edit_start) == new_bytes.get(..edit_start) {
            true => edit_start,
            false => old_bytes
                .iter()
                .zip(new_bytes)
                .take_while(|(a, b)| a == b)
                .count()
                .min(edit_start),
        };
        while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let longest = old.len().min(new.len()) - prefix;
        let mut suffix = match old_bytes[old.len() - longest..] == new_bytes[new.len() - longest..]
        {
            true => longest,
            false => old_bytes
                .iter()
                .rev()
                .zip(new_bytes.iter().rev())
                .take_while(|(a, b)| a == b)
                .count()
                .min(longest),
        };
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
        self.text = new_text;
        self.redo.clear();
        let typing = change.is_typing();
        if let Some(last) = self.undo.last_mut()
            && typing
            && last.typing
            && now.duration_since(last.at) < TYPING_PAUSE
            && join_typing(&mut last.changes[0], &change)
        {
            last.at = now;
            return Some(change);
        }
        self.push_step(vec![change.clone()], typing, now);
        Some(change)
    }

    /// Replaces `range` with `text` as its own undo step, and returns the change to replay.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Change {
        self.replace_each(&[(range, text)]).remove(0)
    }

    /// Accepts `suggestions` (in text order, not overlapping) as one undo step: each puts its
    /// text in place of its range. Returns the changes to replay, in order.
    pub fn accept(&mut self, suggestions: &[Suggestion]) -> Vec<Change> {
        let edits: Vec<(Range<usize>, &str)> = suggestions
            .iter()
            .map(|suggestion| (suggestion.range.clone(), suggestion.text.as_str()))
            .collect();
        self.replace_each(&edits)
    }

    /// Puts each edit's text in place of its range (in text order, not overlapping) as one undo
    /// step, as Replace all does. They are applied from the last back, so each range is still
    /// right when its turn comes. Returns the changes to replay, in order.
    pub fn replace_each(&mut self, edits: &[(Range<usize>, &str)]) -> Vec<Change> {
        let mut changes = Vec::new();
        for (range, text) in edits.iter().rev() {
            let change = Change {
                at: range.start,
                removed: self.text[range.clone()].to_string(),
                inserted: text.to_string(),
            };
            self.apply(&change);
            changes.push(change);
        }
        if !changes.is_empty() {
            self.redo.clear();
            self.push_step(changes.clone(), false, Instant::now());
        }
        changes
    }

    /// Tab over several lines: puts `tab_spaces` spaces at the start of each of `lines`, as one
    /// undo step. Returns one change per line, top line first.
    pub fn indent(&mut self, lines: RangeInclusive<usize>, tab_spaces: usize) -> Vec<Change> {
        let mut changes = Vec::new();
        for line in lines {
            let change = Change {
                at: offset(&self.text, line, 0),
                removed: String::new(),
                inserted: " ".repeat(tab_spaces),
            };
            self.apply(&change);
            changes.push(change);
        }
        self.redo.clear();
        self.push_step(changes.clone(), false, Instant::now());
        changes
    }

    /// Shift+Tab: removes up to `tab_spaces` leading spaces from each of `lines`, as one undo
    /// step. Returns one change per line that had any, top line first.
    pub fn unindent(&mut self, lines: RangeInclusive<usize>, tab_spaces: usize) -> Vec<Change> {
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
            self.push_step(changes.clone(), false, Instant::now());
        }
        changes
    }

    /// Alt+↑ and Alt+↓: swaps `lines` with the line above (`up`) or below, as one undo step.
    /// Returns the change, or nothing at the top or bottom of the text. The newlines stay where
    /// they are and the lines move between them, so a last line without one still has none.
    pub fn move_lines(&mut self, lines: RangeInclusive<usize>, up: bool) -> Vec<Change> {
        let (first, last) = lines.into_inner();
        let (top, bottom) = match up {
            true if first > 0 => (first - 1, last),
            false if last + 1 < self.text.split('\n').count() => (first, last + 1),
            _ => return Vec::new(),
        };
        let at = offset(&self.text, top, 0);
        let mut swapped: Vec<&str> = self.text[at..].split('\n').take(bottom - top + 1).collect();
        let end = at + swapped.iter().map(|line| line.len() + 1).sum::<usize>() - 1;
        match up {
            true => swapped.rotate_left(1),
            false => swapped.rotate_right(1),
        }
        let moved = swapped.join("\n");
        vec![self.replace(at..end, &moved)]
    }

    /// Ctrl+Shift+D: puts a copy of `lines` just below them, as one undo step.
    pub fn duplicate_lines(&mut self, lines: RangeInclusive<usize>) -> Vec<Change> {
        let span = line_span(&self.text, lines);
        let copy = format!("\n{}", &self.text[span.clone()]);
        vec![self.replace(span.end..span.end, &copy)]
    }

    /// Ctrl+Shift+K: deletes `lines` and the newline after them (before them, for the last line)
    /// as one undo step. Returns nothing when there's nothing to delete.
    pub fn delete_lines(&mut self, lines: RangeInclusive<usize>) -> Vec<Change> {
        let span = line_span(&self.text, lines);
        let range = match span.end < self.text.len() {
            true => span.start..span.end + 1,
            false => span.start.saturating_sub(1)..span.end,
        };
        if range.is_empty() {
            return Vec::new();
        }
        vec![self.replace(range, "")]
    }

    /// Undoes the last step. Returns the changes to replay into the widget, in order (empty
    /// when there was nothing to undo).
    pub fn undo(&mut self) -> Vec<Change> {
        let Some(mut step) = self.undo.pop() else {
            return Vec::new();
        };
        let changes: Vec<Change> = step.changes.iter().rev().map(Change::inverse).collect();
        for change in &changes {
            self.apply(change);
        }
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
            self.apply(change);
        }
        let changes = step.changes.clone();
        self.undo.push(step);
        changes
    }

    fn apply(&mut self, change: &Change) {
        self.text
            .replace_range(change.removed_range(), &change.inserted);
    }

    fn push_step(&mut self, changes: Vec<Change>, typing: bool, at: Instant) {
        self.undo.push(Step {
            changes,
            typing,
            at,
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

/// Bytes from the start of the first of `lines` to the end of the last, without its newline.
fn line_span(text: &str, lines: RangeInclusive<usize>) -> Range<usize> {
    let (first, last) = lines.into_inner();
    let start = offset(text, first, 0);
    let length: usize = text[start..]
        .split('\n')
        .take(last - first + 1)
        .map(|line| line.len() + 1)
        .sum();
    start..start + length - 1
}

/// The (line, byte column) of a byte offset in `text`.
pub fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let column = offset - before.rfind('\n').map_or(0, |at| at + 1);
    (line, column)
}

/// The first and last line of `text` that bytes `a` to `b` (either way round) are on. A last
/// line only the start of is in the range doesn't count, as nothing on it is selected.
pub fn selected_lines(text: &str, a: usize, b: usize) -> (usize, usize) {
    let (first, _) = position(text, a.min(b));
    let (last, column) = position(text, a.max(b));
    if column == 0 && last > first {
        (first, last - 1)
    } else {
        (first, last)
    }
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
                widget.replace_range(change.removed_range(), &change.inserted);
            }
            assert_eq!(widget, model.text());
        }
        for change in model.redo() {
            widget.replace_range(change.removed_range(), &change.inserted);
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
    fn indent_adds_one_indent_to_every_line_as_one_step() {
        let mut model = TextModel::new("a\n  b\n\nc".to_string());
        let changes = model.indent(0..=2, 4);
        assert_eq!(model.text(), "    a\n      b\n    \nc");
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[1].at, 6, "after the indent on line 0");
        model.undo();
        assert_eq!(model.text(), "a\n  b\n\nc");
    }

    #[test]
    fn move_lines_swaps_with_the_line_above_or_below_as_one_step() {
        let mut model = TextModel::new("one\ntwo\nthree\n".to_string());
        model.move_lines(1..=1, true);
        assert_eq!(model.text(), "two\none\nthree\n");
        model.move_lines(0..=1, false);
        assert_eq!(model.text(), "three\ntwo\none\n");
        model.undo();
        assert_eq!(model.text(), "two\none\nthree\n");
        model.undo();
        assert_eq!(model.text(), "one\ntwo\nthree\n");
        // At the top or the bottom (the empty line after the last newline) nothing moves, and no
        // undo step is added.
        assert!(model.move_lines(0..=1, true).is_empty());
        assert!(model.move_lines(3..=3, false).is_empty());
        assert!(model.undo().is_empty());
        assert!(
            TextModel::new(String::new())
                .move_lines(0..=0, false)
                .is_empty()
        );
    }

    #[test]
    fn move_lines_leaves_a_missing_last_newline_missing() {
        let mut model = TextModel::new("naïve\ncafé".to_string());
        let changes = model.move_lines(1..=1, true);
        assert_eq!(model.text(), "café\nnaïve");
        let mut widget = "naïve\ncafé".to_string();
        for change in &changes {
            widget.replace_range(change.removed_range(), &change.inserted);
        }
        assert_eq!(widget, model.text());
        model.move_lines(0..=0, false);
        assert_eq!(model.text(), "naïve\ncafé");
    }

    #[test]
    fn duplicate_lines_copies_them_below_as_one_step() {
        let mut model = TextModel::new("one\ntwo\nthree".to_string());
        model.duplicate_lines(0..=1);
        assert_eq!(model.text(), "one\ntwo\none\ntwo\nthree");
        model.undo();
        model.duplicate_lines(2..=2);
        assert_eq!(
            model.text(),
            "one\ntwo\nthree\nthree",
            "the last line, with no newline"
        );
    }

    #[test]
    fn delete_lines_takes_their_newline_too() {
        let mut model = TextModel::new("one\ntwo\nthree".to_string());
        model.delete_lines(0..=1);
        assert_eq!(model.text(), "three");
        model.undo();
        model.delete_lines(2..=2);
        assert_eq!(
            model.text(),
            "one\ntwo",
            "the last line takes the newline before it"
        );
        model.delete_lines(0..=1);
        assert_eq!(model.text(), "");
        assert!(
            model.delete_lines(0..=0).is_empty(),
            "nothing left to delete"
        );
        model.undo();
        assert_eq!(model.text(), "one\ntwo");
    }

    #[test]
    fn selected_lines_leave_out_a_line_only_its_start_is_in() {
        let text = "ab\ncd\nef";
        assert_eq!(selected_lines(text, 1, 1), (0, 0));
        assert_eq!(selected_lines(text, 6, 1), (0, 1), "either way round");
        assert_eq!(
            selected_lines(text, 0, 3),
            (0, 0),
            "the start of line 1 only"
        );
        assert_eq!(selected_lines(text, 3, 3), (1, 1), "the caret alone");
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
    fn accepting_suggestions_is_one_undo_step() {
        let mut model = TextModel::new("Be quick. Stay calm.".to_string());
        let suggestion = |range, text: &str| Suggestion {
            id: 0,
            range,
            text: text.to_string(),
        };
        let changes = model.accept(&[
            suggestion(0..9, "Take your time."),
            suggestion(15..19, "steady"),
        ]);
        assert_eq!(model.text(), "Take your time. Stay steady.");
        // The changes replay in order onto the old text.
        let mut widget = "Be quick. Stay calm.".to_string();
        for change in &changes {
            widget.replace_range(change.removed_range(), &change.inserted);
        }
        assert_eq!(widget, model.text());
        model.undo();
        assert_eq!(model.text(), "Be quick. Stay calm.");
        model.redo();
        assert_eq!(model.text(), "Take your time. Stay steady.");
    }
}
