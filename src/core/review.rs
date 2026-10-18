//! Review mode: edits the assistant suggests, shown inside the text until each one is accepted or
//! rejected. Suggestions never enter the text model, so saving, counts and undo can't contain
//! them.
//!
//! While any are waiting, the editor shows a display string instead of the text: the text with
//! each suggestion spliced in (the text it replaces, struck through, then its new text as ghost
//! text, then room for its Accept and Reject buttons). `DisplayMap` maps offsets between the two.
//! All offsets are bytes, like the rest of `core`.

use std::ops::Range;

use crate::core::text_model::Change;

/// The room after each suggestion where the editor draws its Accept and Reject buttons. The
/// editor draws these characters invisible; they aren't spaces, so the room never breaks across
/// lines and wraps along with the suggestion's last word.
pub const BUTTON_ROOM: &str = "______________";

/// Shown just before each suggestion's new text: a zero-width space, so the new text starts a
/// word of its own. (The editor lays out a plain ASCII word in one font, so its italics could
/// not otherwise start in the middle of a word, as in "phrasing" + ".".)
pub const GHOST_START: &str = "\u{200b}";

/// One suggested edit: put `text` in place of `range`.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Stays the same while the text around it changes, so a button or a chat card can name it.
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

    /// Moves the suggestions to follow a change to the text (an accepted suggestion, an undo).
    /// One the change overlaps is dropped, since the text it was about has gone.
    pub fn shift(&mut self, change: &Change) {
        let removed_end = change.at + change.removed.len();
        self.suggestions.retain_mut(|s| {
            if s.range.end <= change.at {
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
    /// Text a suggestion would replace, shown struck through.
    Struck,
    /// A suggestion's new text.
    Ghost,
    /// The room for a suggestion's buttons.
    Buttons,
}

/// One piece of the display string.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub shown: Shown,
    /// Where it is in the display string.
    pub display: Range<usize>,
    /// Where it is in the text: the text it shows, or for ghost text and buttons the empty range
    /// where they sit (the end of what the suggestion replaces).
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
            map.push(Shown::Struck, &text[range.clone()], range, id);
            let ghost = match suggestion.text.is_empty() {
                true => String::new(),
                false => format!("{GHOST_START}{}", suggestion.text),
            };
            map.push(Shown::Ghost, &ghost, end.clone(), id);
            map.push(Shown::Buttons, BUTTON_ROOM, end, id);
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
    /// suggestion's ghost text sits goes before it, or after its buttons when `after` is set
    /// (where `Pane` puts the caret).
    pub fn to_display(&self, offset: usize, after: bool) -> usize {
        let mut found = None;
        for piece in &self.pieces {
            let shows_text = matches!(piece.shown, Shown::Text | Shown::Struck);
            if shows_text && piece.text.start <= offset && offset <= piece.text.end {
                found = Some(piece.display.start + offset - piece.text.start);
                if !after {
                    break;
                }
            }
        }
        found.unwrap_or(0)
    }

    /// Where display offset `offset` is in the text. Inside ghost text or buttons, that's where
    /// the suggestion sits.
    pub fn to_text(&self, offset: usize) -> usize {
        for piece in &self.pieces {
            if piece.display.contains(&offset) {
                return match piece.shown {
                    Shown::Text | Shown::Struck => piece.text.start + offset - piece.display.start,
                    Shown::Ghost | Shown::Buttons => piece.text.start,
                };
            }
        }
        self.pieces.last().map_or(0, |piece| piece.text.end)
    }

    /// The suggestion display offset `offset` (the caret) is on: in or touching its struck text,
    /// ghost text or buttons. `None` in plain text away from every suggestion.
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

    const ROOM: &str = BUTTON_ROOM;
    const Z: &str = GHOST_START;

    fn review_of(text: &str, edits: &[(Range<usize>, &str)]) -> Review {
        let mut review = Review::default();
        for (range, new_text) in edits {
            review.add(text, range.clone(), new_text).unwrap();
        }
        review
    }

    #[test]
    fn the_display_splices_in_struck_ghost_and_button_room() {
        let text = "Keep calm. Be quick.";
        // Replace "Be quick." and insert before "Keep".
        let review = review_of(text, &[(11..20, "Take your time."), (0..0, "Breathe. ")]);
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(
            map.text,
            format!("{Z}Breathe. {ROOM}Keep calm. Be quick.{Z}Take your time.{ROOM}")
        );
        let shown: Vec<(Shown, &str)> = map
            .pieces
            .iter()
            .filter(|piece| !piece.display.is_empty())
            .map(|piece| (piece.shown, &map.text[piece.display.clone()]))
            .collect();
        assert_eq!(
            shown,
            [
                (Shown::Ghost, "\u{200b}Breathe. "),
                (Shown::Buttons, ROOM),
                (Shown::Text, "Keep calm. "),
                (Shown::Struck, "Be quick."),
                (Shown::Ghost, "\u{200b}Take your time."),
                (Shown::Buttons, ROOM),
            ]
        );
    }

    #[test]
    fn offsets_map_both_ways() {
        let text = "one two three";
        let review = review_of(text, &[(4..7, "2")]);
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(map.text, format!("one two{Z}2{ROOM} three"));
        let after_room = 8 + Z.len() + ROOM.len();
        // Plain and struck text keep their place; the end of the struck word can sit on either
        // side of the ghost text.
        assert_eq!(map.to_display(2, false), 2);
        assert_eq!(map.to_display(5, false), 5);
        assert_eq!(map.to_display(7, false), 7);
        assert_eq!(map.to_display(7, true), after_room);
        assert_eq!(map.to_display(9, true), after_room + 2);
        assert_eq!(map.to_display(text.len(), false), map.text.len());
        // Back again: inside the ghost text or the button room is the suggestion's end.
        assert_eq!(map.to_text(5), 5);
        assert_eq!(
            map.to_text(7),
            7,
            "the ghost text sits at the end of the struck word"
        );
        assert_eq!(map.to_text(after_room - 1), 7);
        assert_eq!(map.to_text(after_room + 2), 9);
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
        assert_eq!(map.text, format!("a{Z}X{ROOM}b"));
        assert_eq!(map.to_display(1, false), 1);
        assert_eq!(map.to_display(1, true), 2 + Z.len() + ROOM.len());

        let review = review_of(text, &[(0..1, "")]);
        let map = DisplayMap::new(text, review.suggestions());
        assert_eq!(map.text, format!("a{ROOM}b"));
        assert_eq!(map.to_text(0), 0);
        assert_eq!(map.to_text(1 + ROOM.len()), 1);
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
        // On the struck text, inside the ghost text, and right after the buttons.
        assert_eq!(map.suggestion_at(2), Some(ids[0]));
        assert_eq!(
            map.suggestion_at(map.to_display(4, false) + Z.len()),
            Some(ids[0])
        );
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
        review.shift(&Change {
            at: 4,
            removed: "two".into(),
            inserted: "2".into(),
        });
        let ranges: Vec<Range<usize>> = review
            .suggestions()
            .iter()
            .map(|s| s.range.clone())
            .collect();
        assert_eq!(ranges, [0..3, 6..11, 12..16]);
        // A change inside "three" drops that suggestion only.
        review.shift(&Change {
            at: 7,
            removed: "h".into(),
            inserted: "H".into(),
        });
        let ranges: Vec<Range<usize>> = review
            .suggestions()
            .iter()
            .map(|s| s.range.clone())
            .collect();
        assert_eq!(ranges, [0..3, 12..16]);
        // Typing right at a suggestion's start pushes it along; right at its end leaves it.
        review.shift(&Change {
            at: 12,
            removed: "".into(),
            inserted: "xx".into(),
        });
        review.shift(&Change {
            at: 3,
            removed: "".into(),
            inserted: "y".into(),
        });
        let ranges: Vec<Range<usize>> = review
            .suggestions()
            .iter()
            .map(|s| s.range.clone())
            .collect();
        assert_eq!(ranges, [0..3, 15..19]);
    }
}
