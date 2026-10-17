//! Paint: which stretches of text belong to which idea, and how those stretches follow edits.
//!
//! A version's paint is a flat list of `Span`s in byte offsets. Each span is independent, so two
//! ideas on the same words are two spans and overlaps cost nothing. `core::text_model` keeps the
//! draft's paint in its undo history; the manifest stores each version's paint and the draft's.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::core::text_model::Change;

/// Which idea a span belongs to. A plain number, stored in the manifest.
pub type IdeaId = u32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub idea: IdeaId,
    /// Byte offsets into the version's text; `start < end`.
    pub start: usize,
    pub end: usize,
    /// The painted text as last saved, so the span can be found again after an outside edit.
    pub quote: String,
}

/// Moves every span to follow `change`, and drops spans left empty (that idea then shows as
/// missing).
///
/// - A position before the change stays; one after it moves by the change's length difference.
/// - A position inside the replaced text goes to the edge of the new text: a start to its start,
///   an end to its end. So retyping a word inside painted text keeps it painted.
/// - A span wholly inside the replaced text goes with it, unless it is exactly that text. So
///   pasting over a paragraph never hands its ideas the pasted text.
/// - Inserting exactly at a span's edge does not grow the span (but see `continue_word`).
pub fn shift(spans: &mut Vec<Span>, change: &Change) {
    let removed_end = change.at + change.removed.len();
    let inserted_end = change.at + change.inserted.len();
    let moved = |position: usize| position + change.inserted.len() - change.removed.len();
    spans.retain(|span| {
        let inside = change.at <= span.start && span.end <= removed_end;
        !inside || (span.start, span.end) == (change.at, removed_end)
    });
    for span in spans.iter_mut() {
        span.start = if span.start < change.at {
            span.start
        } else if span.start >= removed_end {
            moved(span.start)
        } else {
            change.at
        };
        span.end = if span.end <= change.at {
            span.end
        } else if span.end >= removed_end {
            moved(span.end)
        } else {
            inserted_end
        };
    }
    spans.retain(|span| span.start < span.end);
}

/// After a pure insert of text with no spaces straight after a painted word, grows the spans that
/// end there over it. So retyping a span's last word one key at a time keeps it painted.
/// `text` is the text after the insert.
pub fn continue_word(spans: &mut [Span], change: &Change, text: &str) {
    let before = text[..change.at].chars().next_back();
    if !change.removed.is_empty()
        || change.inserted.contains(char::is_whitespace)
        || before.is_none_or(char::is_whitespace)
    {
        return;
    }
    for span in spans {
        if span.end == change.at && span.start < change.at {
            span.end += change.inserted.len();
        }
    }
}

/// Paints `range` with `idea`, joining any span of the same idea it touches or overlaps.
pub fn brush(spans: &mut Vec<Span>, range: Range<usize>, idea: IdeaId) {
    if range.is_empty() {
        return;
    }
    let mut painted = Span {
        idea,
        start: range.start,
        end: range.end,
        quote: String::new(),
    };
    spans.retain(|span| {
        let touches = span.start <= painted.end && painted.start <= span.end;
        if span.idea == idea && touches {
            painted.start = painted.start.min(span.start);
            painted.end = painted.end.max(span.end);
            false
        } else {
            true
        }
    });
    spans.push(painted);
}

/// Takes `idea`'s paint off `range`, cutting any of its spans that reach past either edge.
pub fn unpaint(spans: &mut Vec<Span>, range: Range<usize>, idea: IdeaId) {
    let mut right_pieces = Vec::new();
    spans.retain_mut(|span| {
        if span.idea != idea || span.end <= range.start || range.end <= span.start {
            return true;
        }
        if span.end > range.end {
            right_pieces.push(Span {
                start: range.end,
                ..span.clone()
            });
        }
        span.end = range.start;
        span.start < range.start
    });
    spans.extend(right_pieces);
}

/// Moves all of `from`'s paint to `to`, joining it with `to`'s own where they touch.
pub fn relabel(spans: &mut Vec<Span>, from: IdeaId, to: IdeaId) {
    let moved: Vec<Range<usize>> = spans
        .iter()
        .filter(|span| span.idea == from)
        .map(|span| span.start..span.end)
        .collect();
    spans.retain(|span| span.idea != from);
    for range in moved {
        brush(spans, range, to);
    }
}

/// Sets every span's `quote` to the text it covers now. Call before saving the manifest.
pub fn quotes(spans: &mut [Span], text: &str) {
    for span in spans {
        span.quote = text.get(span.start..span.end).unwrap_or("").to_string();
    }
}

/// After the text changed outside the app: keeps spans whose `quote` still sits where it was,
/// moves the others to the copy of their quote nearest the old place, and drops the ones whose
/// quote is gone (the idea then shows as missing). Nothing is attached to the wrong text.
pub fn relocate(spans: &mut Vec<Span>, text: &str) {
    spans.retain_mut(|span| {
        if span.quote.is_empty() {
            return false;
        }
        if text.get(span.start..span.end) == Some(span.quote.as_str()) {
            return true;
        }
        let nearest = text
            .match_indices(span.quote.as_str())
            .map(|(at, _)| at)
            .min_by_key(|at| at.abs_diff(span.start));
        match nearest {
            Some(at) => {
                span.start = at;
                span.end = at + span.quote.len();
                true
            }
            None => false,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(idea: IdeaId, start: usize, end: usize) -> Span {
        Span {
            idea,
            start,
            end,
            quote: String::new(),
        }
    }

    fn change(at: usize, removed: &str, inserted: &str) -> Change {
        Change {
            at,
            removed: removed.to_string(),
            inserted: inserted.to_string(),
        }
    }

    fn shifted(start: usize, end: usize, change: &Change) -> Option<(usize, usize)> {
        let mut spans = vec![span(1, start, end)];
        shift(&mut spans, change);
        spans.first().map(|span| (span.start, span.end))
    }

    #[test]
    fn edits_before_a_span_move_it_and_edits_after_leave_it() {
        assert_eq!(shifted(10, 20, &change(2, "", "abc")), Some((13, 23)));
        assert_eq!(shifted(10, 20, &change(2, "abcd", "")), Some((6, 16)));
        assert_eq!(shifted(10, 20, &change(25, "", "abc")), Some((10, 20)));
        assert_eq!(shifted(10, 20, &change(20, "xyz", "")), Some((10, 20)));
    }

    #[test]
    fn typing_inside_grows_the_span_but_typing_at_its_edges_does_not() {
        assert_eq!(shifted(10, 20, &change(15, "", "abc")), Some((10, 23)));
        assert_eq!(shifted(10, 20, &change(10, "", "abc")), Some((13, 23)));
        assert_eq!(shifted(10, 20, &change(20, "", "abc")), Some((10, 20)));
    }

    #[test]
    fn deleting_inside_shrinks_the_span() {
        assert_eq!(shifted(10, 20, &change(12, "abc", "")), Some((10, 17)));
        // Deleting across the start: the start moves to where the deletion was.
        assert_eq!(shifted(10, 20, &change(5, "0123456789", "")), Some((5, 10)));
        // Deleting across the end: the end comes back to where the deletion was.
        assert_eq!(
            shifted(10, 20, &change(15, "0123456789", "")),
            Some((10, 15))
        );
    }

    #[test]
    fn retyping_a_word_inside_keeps_it_painted() {
        // "the quick fox" painted whole; "quick" (4..9) becomes "slow".
        assert_eq!(shifted(0, 13, &change(4, "quick", "slow")), Some((0, 12)));
        // A replacement reaching past the end ends the span after the new text.
        assert_eq!(shifted(0, 6, &change(4, "quick", "slow")), Some((0, 8)));
        // One reaching before the start starts the span at the new text.
        assert_eq!(shifted(6, 13, &change(4, "quick", "slow")), Some((4, 12)));
    }

    #[test]
    fn deleting_all_of_a_span_drops_it() {
        assert_eq!(shifted(10, 20, &change(10, "0123456789", "")), None);
        assert_eq!(
            shifted(10, 20, &change(5, "x".repeat(20).as_str(), "")),
            None
        );
        // Replacing exactly the painted text keeps the paint on the new text.
        assert_eq!(
            shifted(10, 20, &change(10, "0123456789", "new")),
            Some((10, 13))
        );
    }

    #[test]
    fn pasting_over_whole_spans_drops_them() {
        // "one two three four": ideas 1 and 2 paint "two" and "three"; "two three" and the
        // spaces around it are pasted over.
        let mut spans = vec![span(1, 4, 7), span(2, 8, 13), span(3, 0, 18)];
        shift(&mut spans, &change(3, " two three ", " pasted text "));
        assert_eq!(spans, vec![span(3, 0, 20)], "only the span around it stays");
        // Typing one letter over them is the same.
        let mut spans = vec![span(1, 4, 7), span(2, 8, 13)];
        shift(&mut spans, &change(3, " two three ", "X"));
        assert!(spans.is_empty());
    }

    #[test]
    fn typing_on_from_a_painted_word_keeps_painting_it() {
        // "calm but bold": " but bold" (4..13) is painted; "bold" is selected and "brave" typed
        // one key at a time.
        let mut text = "calm but bold".to_string();
        let mut spans = vec![span(1, 4, 13)];
        let mut at = 9;
        for (n, letter) in ["b", "r", "a", "v", "e"].into_iter().enumerate() {
            let removed = if n == 0 { "bold" } else { "" };
            let change = change(at, removed, letter);
            text.replace_range(at..at + removed.len(), letter);
            shift(&mut spans, &change);
            continue_word(&mut spans, &change, &text);
            at += letter.len();
        }
        assert_eq!(text, "calm but brave");
        assert_eq!(spans, vec![span(1, 4, 14)]);
        // A space, or typing after a painted space, starts unpainted text.
        for (text, at, inserted) in [("ab x", 2, " x"), ("ab x", 3, "x")] {
            let mut spans = vec![span(1, 0, at)];
            continue_word(&mut spans, &change(at, "", inserted), text);
            assert_eq!(spans, vec![span(1, 0, at)]);
        }
    }

    #[test]
    fn overlapping_spans_move_independently() {
        let mut spans = vec![span(1, 0, 10), span(2, 5, 15)];
        shift(&mut spans, &change(7, "", "ab"));
        assert_eq!(spans, vec![span(1, 0, 12), span(2, 5, 17)]);
    }

    #[test]
    fn brush_joins_touching_spans_of_the_same_idea_only() {
        let mut spans = vec![span(1, 0, 5), span(2, 5, 9), span(1, 12, 20)];
        brush(&mut spans, 5..8, 1);
        assert_eq!(spans, vec![span(2, 5, 9), span(1, 12, 20), span(1, 0, 8)]);
        // Bridging the gap joins all three pieces of idea 1.
        brush(&mut spans, 8..12, 1);
        assert_eq!(spans, vec![span(2, 5, 9), span(1, 0, 20)]);
        brush(&mut spans, 3..3, 2);
        assert_eq!(spans.len(), 2, "an empty range paints nothing");
    }

    #[test]
    fn quotes_record_the_painted_text() {
        let mut spans = vec![span(1, 4, 9)];
        quotes(&mut spans, "the quick fox");
        assert_eq!(spans[0].quote, "quick");
    }

    #[test]
    fn relocate_keeps_moves_or_drops_by_quote() {
        let text = "one two three two";
        let mut kept = vec![span(1, 4, 7)];
        quotes(&mut kept, text);
        relocate(&mut kept, text);
        assert_eq!((kept[0].start, kept[0].end), (4, 7));

        // Text was inserted at the front outside the app: "two" is now further on. Of its two
        // copies, the one nearest the old place wins.
        relocate(&mut kept, "zero one two three two");
        assert_eq!((kept[0].start, kept[0].end), (9, 12));

        relocate(&mut kept, "nothing here");
        assert!(kept.is_empty());
    }

    #[test]
    fn spans_round_trip_through_json() {
        let spans = vec![Span {
            idea: 3,
            start: 0,
            end: 7,
            quote: "The fox".to_string(),
        }];
        let json = serde_json::to_string(&spans).unwrap();
        assert_eq!(json, r#"[{"idea":3,"start":0,"end":7,"quote":"The fox"}]"#);
        assert_eq!(serde_json::from_str::<Vec<Span>>(&json).unwrap(), spans);
    }

    #[test]
    fn unpaint_cuts_only_that_idea_out_of_the_range() {
        let mut spans = vec![span(1, 0, 10), span(2, 0, 10), span(1, 12, 14)];
        unpaint(&mut spans, 3..13, 1);
        assert_eq!(
            spans,
            vec![span(1, 0, 3), span(2, 0, 10), span(1, 13, 14)],
            "the left piece stays in place, the right piece is added"
        );
        // A range in the middle of a span leaves both ends.
        let mut spans = vec![span(1, 0, 10)];
        unpaint(&mut spans, 4..6, 1);
        assert_eq!(spans, vec![span(1, 0, 4), span(1, 6, 10)]);
        unpaint(&mut spans, 0..10, 1);
        assert!(spans.is_empty());
    }

    #[test]
    fn relabel_moves_paint_and_joins_it() {
        let mut spans = vec![span(1, 0, 5), span(2, 5, 9), span(3, 0, 2)];
        relabel(&mut spans, 2, 1);
        assert_eq!(spans, vec![span(3, 0, 2), span(1, 0, 9)]);
    }

    /// A tiny random number generator, so the random tests need no extra crate and always run
    /// the same way.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as usize) % n.max(1)
        }

        /// A char boundary in `text`, picked at random.
        fn boundary(&mut self, text: &str) -> usize {
            let boundaries: Vec<usize> = (0..=text.len())
                .filter(|&i| text.is_char_boundary(i))
                .collect();
            boundaries[self.below(boundaries.len())]
        }

        fn words(&mut self) -> String {
            const PIECES: [&str; 6] = ["a", "fox ", "夜", "é", "\n", "brown "];
            (0..self.below(4)).map(|_| PIECES[self.below(6)]).collect()
        }
    }

    #[test]
    fn random_edits_keep_spans_valid_and_text_outside_the_edit_painted() {
        let mut random = Lcg(7);
        for _ in 0..200 {
            let mut text = "the quick 夜 brown fox é jumps\nover the lazy dog".to_string();
            let mut spans = Vec::new();
            for idea in 1..4 {
                let a = random.boundary(&text);
                let b = random.boundary(&text);
                brush(&mut spans, a.min(b)..a.max(b), idea);
            }
            for _ in 0..30 {
                let a = random.boundary(&text);
                let b = random.boundary(&text);
                let (at, end) = (a.min(b), a.max(b));
                let change = change(at, &text[at..end], &random.words());
                quotes(&mut spans, &text);
                let before = spans.clone();
                text.replace_range(at..end, &change.inserted);
                shift(&mut spans, &change);

                for span in &spans {
                    assert!(span.start < span.end && span.end <= text.len());
                    assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
                }
                // A span clear of the replaced text, or wrapped around it, keeps its words.
                for old in &before {
                    let new = spans.iter().find(|span| span.idea == old.idea);
                    let painted = |span: &Span| text[span.start..span.end].to_string();
                    if old.end < at || old.start > end {
                        assert_eq!(new.map(painted), Some(old.quote.clone()));
                    } else if old.start < at && end < old.end {
                        let expected = format!(
                            "{}{}{}",
                            &old.quote[..at - old.start],
                            change.inserted,
                            &old.quote[end - old.start..]
                        );
                        assert_eq!(new.map(painted), Some(expected));
                    }
                }
            }
        }
    }

    #[test]
    fn random_brushing_and_unpainting_match_a_byte_map() {
        let mut random = Lcg(11);
        let mut spans = Vec::new();
        let mut painted = [false; 40];
        for _ in 0..2000 {
            let a = random.below(41);
            let b = random.below(41);
            let range = a.min(b)..a.max(b);
            let paint = random.below(2) == 0;
            match paint {
                true => brush(&mut spans, range.clone(), 1),
                false => unpaint(&mut spans, range.clone(), 1),
            }
            painted[range].fill(paint);
            let from_spans: Vec<bool> = (0..40)
                .map(|i| spans.iter().any(|span| span.start <= i && i < span.end))
                .collect();
            assert_eq!(from_spans, painted);
            assert!(spans.iter().all(|span| span.start < span.end));
        }
    }
}
