//! Review mode: edits the assistant suggests, shown inside the text until each one is accepted or
//! rejected. Suggestions never enter the text model, so saving, counts and undo can't contain
//! them.
//!
//! While any are waiting, the editor shows a display string instead of the text: the text with
//! each suggestion spliced in (the text it takes out, then the text it puts in, which the editor
//! colours red and green). `DisplayMap` maps offsets between the two. All offsets are bytes, like
//! the rest of `core`.

use std::ops::Range;

use crate::core::text_model::Change;

/// One suggested edit: put `text` in place of `range`.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Stays the same while the text around it changes, so a chat card can name it.
    pub id: u64,
    /// The bytes of the text it replaces; empty for a pure insert.
    pub range: Range<usize>,
    /// What it puts there; empty for a pure delete.
    pub text: String,
}

/// The suggestions waiting in one pane, in text order and never overlapping.
#[derive(Debug, Default)]
pub struct Review {
    suggestions: Vec<Suggestion>,
    last_id: u64,
}

impl Review {
    pub fn suggestions(&self) -> &[Suggestion] {
        &self.suggestions
    }

    /// Adds a suggestion to replace `range` of `text` with `new_text`, and returns its id.
    /// Refused, with the reason, when the range is outside the text or splits a character,
    /// changes nothing, or overlaps (or starts at the same place as) another suggestion.
    pub fn add(&mut self, text: &str, range: Range<usize>, new_text: &str) -> Result<u64, String> {
        if range.start > range.end
            || range.end > text.len()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            return Err("That part of the text isn't there any more".to_string());
        }
        if text[range.clone()] == *new_text {
            return Err("The suggestion doesn't change anything".to_string());
        }
        if self
            .suggestions
            .iter()
            .any(|other| clashes(&other.range, &range))
        {
            return Err("Another suggestion already changes that part of the text".to_string());
        }
        self.last_id += 1;
        let at = self
            .suggestions
            .iter()
            .take_while(|other| other.range.start < range.start)
            .count();
        self.suggestions.insert(
            at,
            Suggestion {
                id: self.last_id,
                range,
                text: new_text.to_string(),
            },
        );
        Ok(self.last_id)
    }

    /// Takes suggestion `id` out, to accept or reject it.
    pub fn take(&mut self, id: u64) -> Option<Suggestion> {
        let index = self.suggestions.iter().position(|s| s.id == id)?;
        Some(self.suggestions.remove(index))
    }

    /// Takes every suggestion out, in text order.
    pub fn take_all(&mut self) -> Vec<Suggestion> {
        std::mem::take(&mut self.suggestions)
    }

    /// Moves the suggestions to follow a change to the text (typing around them, an accepted
    /// suggestion, an undo). One the change overlaps is dropped, since the text it was about has
    /// gone. A pure insert right where the change is stays before it, unless `ahead`: the change
    /// was typed just before the insert's new text, so the insert moves along after it.
    pub fn shift(&mut self, change: &Change, ahead: bool) {
        let removed_end = change.at + change.removed.len();
        self.suggestions.retain_mut(|s| {
            let pushed = ahead && s.range == (change.at..change.at);
            if s.range.end <= change.at && !pushed {
                true
            } else if s.range.start >= removed_end {
                s.range.start = s.range.start - removed_end + change.at + change.inserted.len();
                s.range.end = s.range.end - removed_end + change.at + change.inserted.len();
                true
            } else {
                false
            }
        });
    }
}

/// Whether two suggestions for these ranges of the text would get in each other's way: they
/// overlap, or start at the same place.
pub fn clashes(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start == b.start || (a.start < b.end && b.start < a.end)
}

/// What a piece of the display string shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shown {
    /// The text as it is.
    Text,
    /// Text a suggestion takes out.
    Removed,
    /// Text a suggestion puts in.
    Added,
}

/// One piece of the display string.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub shown: Shown,
    /// Where it is in the display string.
    pub display: Range<usize>,
    /// Where it is in the text: the text it shows, or for added text the empty range where it
    /// sits (the end of what the suggestion takes out).
    pub text: Range<usize>,
    /// The suggestion it belongs to; `None` for plain text.
    pub suggestion: Option<u64>,
}

/// The display string and how its pieces map back to the text.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayMap {
    pub text: String,
    /// In order, covering the display string. A plain-text piece comes before each suggestion
    /// and one at the end, even when empty.
    pub pieces: Vec<Piece>,
}

impl DisplayMap {
    /// `text` with `suggestions` (in text order, not overlapping) spliced in.
    pub fn new(text: &str, suggestions: &[Suggestion]) -> DisplayMap {
        let mut map = DisplayMap {
            text: String::new(),
            pieces: Vec::new(),
        };
        let mut at = 0;
        for suggestion in suggestions {
            let range = suggestion.range.clone();
            let end = range.end..range.end;
            let id = Some(suggestion.id);
            map.push(Shown::Text, &text[at..range.start], at..range.start, None);
            map.push(Shown::Removed, &text[range.clone()], range, id);
            map.push(Shown::Added, &suggestion.text, end, id);
            at = suggestion.range.end;
        }
        map.push(Shown::Text, &text[at..], at..text.len(), None);
        map
    }

    fn push(&mut self, shown: Shown, words: &str, text: Range<usize>, suggestion: Option<u64>) {
        let start = self.text.len();
        self.text.push_str(words);
        self.pieces.push(Piece {
            shown,
            display: start..self.text.len(),
            text,
            suggestion,
        });
    }

    /// Where byte `offset` of the text is in the display string. An offset right where a
    /// suggestion's added text sits goes before it, or after it when `after` is set (where
    /// `Pane` puts the caret).
    pub fn to_display(&self, offset: usize, after: bool) -> usize {
        let mut found = None;
        for piece in &self.pieces {
            let shows_text = matches!(piece.shown, Shown::Text | Shown::Removed);
            if shows_text && piece.text.start <= offset && offset <= piece.text.end {
                found = Some(piece.display.start + offset - piece.text.start);
                if !after {
                    break;
                }
            }
        }
        found.unwrap_or(0)
    }

    /// Where display offset `offset` is in the text. Inside added text, that's where the
    /// suggestion sits.
    pub fn to_text(&self, offset: usize) -> usize {
        for piece in &self.pieces {
            if piece.display.contains(&offset) {
                return match piece.shown {
                    Shown::Text | Shown::Removed => piece.text.start + offset - piece.display.start,
                    Shown::Added => piece.text.start,
                };
            }
        }
        self.pieces.last().map_or(0, |piece| piece.text.end)
    }

    /// The suggestion display offset `offset` (the caret) is on: in or touching the text it takes
    /// out or puts in. `None` in plain text away from every suggestion.
    pub fn suggestion_at(&self, offset: usize) -> Option<u64> {
        self.pieces.iter().find_map(|piece| {
            let touches = piece.display.start <= offset && offset <= piece.display.end;
            piece.suggestion.filter(|_| touches)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review_of(text: &str, edits: &[(Range<usize>, &str)]) -> Review {
        let mut review = Review::default();
        for (range, new_text) in edits {
            review.add(text, range.clone(), new_text).unwrap();
        }
        review
    }

    #[test]
    fn the_display_splices_in_removed_and_added_text() {
        let text = "Keep calm. Be quick.";
        // Replace "Be quick." and insert before "Keep".
        let review = review_of(text, &[(11..20, "Take your time."), (0..0, "Breathe. ")]);
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(map.text, "Breathe. Keep calm. Be quick.Take your time.");
        let shown: Vec<(Shown, &str)> = map
            .pieces
            .iter()
            .filter(|piece| !piece.display.is_empty())
            .map(|piece| (piece.shown, &map.text[piece.display.clone()]))
            .collect();
        assert_eq!(
            shown,
            [
                (Shown::Added, "Breathe. "),
                (Shown::Text, "Keep calm. "),
                (Shown::Removed, "Be quick."),
                (Shown::Added, "Take your time."),
            ]
        );
    }

    #[test]
    fn offsets_map_both_ways() {
        let text = "one two three";
        let review = review_of(text, &[(4..7, "2")]);
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(map.text, "one two2 three");
        let after_added = 8;
        // Plain and removed text keep their place; the end of the removed word can sit on either
        // side of the added text.
        assert_eq!(map.to_display(2, false), 2);
        assert_eq!(map.to_display(5, false), 5);
        assert_eq!(map.to_display(7, false), 7);
        assert_eq!(map.to_display(7, true), after_added);
        assert_eq!(map.to_display(9, true), after_added + 2);
        assert_eq!(map.to_display(text.len(), false), map.text.len());
        // Back again: inside the added text is the suggestion's end.
        assert_eq!(map.to_text(5), 5);
        assert_eq!(
            map.to_text(7),
            7,
            "the added text sits at the end of the removed word"
        );
        assert_eq!(map.to_text(after_added - 1), 7);
        assert_eq!(map.to_text(after_added + 2), 9);
        assert_eq!(map.to_text(map.text.len()), text.len());
    }

    #[test]
    fn a_pure_insert_and_a_pure_delete() {
        let text = "ab";
        let mut review = review_of(text, &[(1..1, "X")]);
        assert!(
            review.add(text, 1..2, "").is_err(),
            "two suggestions can't start at the same place"
        );
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(map.text, "aXb");
        assert_eq!(map.to_display(1, false), 1);
        assert_eq!(map.to_display(1, true), 2);

        let review = review_of(text, &[(0..1, "")]);
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(map.text, "ab");
        assert_eq!(map.to_text(0), 0);
        assert_eq!(map.to_text(1), 1);
    }

    #[test]
    fn bad_suggestions_are_refused() {
        let text = "café au lait";
        let mut review = review_of(text, &[(0..5, "tea")]);
        assert!(review.add(text, 2..9, "x").is_err(), "overlaps");
        assert!(review.add(text, 0..0, "x").is_err(), "same start");
        assert!(review.add(text, 4..4, "x").is_err(), "splits é");
        assert!(review.add(text, 6..40, "x").is_err(), "past the end");
        assert!(review.add(text, 6..8, "au").is_err(), "changes nothing");
        assert!(review.add(text, 5..5, "!").is_ok(), "right after is fine");
        assert_eq!(review.suggestions()[1].range, 5..5);
    }

    #[test]
    fn the_suggestion_at_the_caret() {
        let text = "aaaa bbbb cccc";
        let review = review_of(text, &[(10..14, "C"), (0..4, "A")]);
        let map = DisplayMap::new(text, review.suggestions());
        let ids: Vec<u64> = review.suggestions().iter().map(|s| s.id).collect();
        // On the removed text, inside the added text, and right after it.
        assert_eq!(map.suggestion_at(2), Some(ids[0]));
        assert_eq!(map.suggestion_at(map.to_display(4, false)), Some(ids[0]));
        assert_eq!(map.suggestion_at(map.to_display(4, true)), Some(ids[0]));
        assert_eq!(map.suggestion_at(map.text.len()), Some(ids[1]));
        // In "bbbb", away from both.
        assert_eq!(map.suggestion_at(map.to_display(7, false)), None);
        assert_eq!(DisplayMap::new(text, &[]).suggestion_at(0), None);
    }

    #[test]
    fn changes_move_later_suggestions_and_drop_overlapped_ones() {
        let text = "one two three four";
        let mut review = review_of(text, &[(0..3, "1"), (8..13, "3"), (14..18, "4")]);
        // "two" becomes "2": the two suggestions after it move back by 2.
        review.shift(
            &Change {
                at: 4,
                removed: "two".into(),
                inserted: "2".into(),
            },
            false,
        );
        let ranges: Vec<Range<usize>> = review
            .suggestions()
            .iter()
            .map(|s| s.range.clone())
            .collect();
        assert_eq!(ranges, [0..3, 6..11, 12..16]);
        // A change inside "three" drops that suggestion only.
        review.shift(
            &Change {
                at: 7,
                removed: "h".into(),
                inserted: "H".into(),
            },
            false,
        );
        let ranges: Vec<Range<usize>> = review
            .suggestions()
            .iter()
            .map(|s| s.range.clone())
            .collect();
        assert_eq!(ranges, [0..3, 12..16]);
        // Typing right at a suggestion's start pushes it along; right at its end leaves it.
        review.shift(
            &Change {
                at: 12,
                removed: "".into(),
                inserted: "xx".into(),
            },
            false,
        );
        review.shift(
            &Change {
                at: 3,
                removed: "".into(),
                inserted: "y".into(),
            },
            false,
        );
        let ranges: Vec<Range<usize>> = review
            .suggestions()
            .iter()
            .map(|s| s.range.clone())
            .collect();
        assert_eq!(ranges, [0..3, 15..19]);
        // A pure insert stays before typing right where it sits, unless that was typed ahead of
        // its added text.
        let mut review = review_of("ab", &[(1..1, "X")]);
        let typed = Change {
            at: 1,
            removed: "".into(),
            inserted: "!".into(),
        };
        review.shift(&typed, false);
        assert_eq!(review.suggestions()[0].range, 1..1);
        review.shift(&typed, true);
        assert_eq!(review.suggestions()[0].range, 2..2);
    }
}
