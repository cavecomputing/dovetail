//! Paint: which stretches of text belong to which idea, and how those stretches follow edits.
//!
//! A version's paint is a flat list of `Span`s in byte offsets. Each span is independent, so two
//! ideas on the same words are two spans and overlaps cost nothing.
//!
//! The design (docs/editor-design.md) puts this in `project/paint.rs`; it lives in `core/` for now
//! because `core::text_model` keeps paint in its undo history and `project/` is still being built.

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
/// - Inserting exactly at a span's edge does not grow the span.
pub fn shift(spans: &mut Vec<Span>, change: &Change) {
    let removed_end = change.at + change.removed.len();
    let inserted_end = change.at + change.inserted.len();
    let moved = |position: usize| position + change.inserted.len() - change.removed.len();
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
}
