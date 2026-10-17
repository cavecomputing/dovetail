//! Idea weights: how much of the text each idea covers, old version against draft, and the
//! Commit check built from them (spec section 7 and 8).
//!
//! An idea's text is the union of its spans, so its own overlapping spans count once. Text painted
//! with two ideas counts toward both, so the shares can add up to more than 100%. A word or token
//! counts where it starts, so stretches that split the text add up to exactly the whole.

use std::ops::Range;

use crate::core::counts::{Counts, token_starts};
use crate::core::markup::without_markup;
use crate::project::ideas::TopicId;
use crate::project::manifest::Manifest;
use crate::project::paint::{IdeaId, Span};

/// One row of the weights view: how much text, old and new, and its share of the whole text.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Weight {
    pub old: Counts,
    pub new: Counts,
    /// Percent of the whole text, by tokens once the tokenizer is ready, by words until then.
    pub old_share: f64,
    pub new_share: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// Has text in the draft.
    Carried,
    /// No text in the draft, and marked as cut on purpose in a version after the old one (or
    /// ever, when the old version has none of it either).
    Cut,
    /// Had text in the old version, none in the draft, and not marked as cut.
    Missing,
    /// No text in either.
    Unused,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdeaWeight {
    pub idea: IdeaId,
    pub weight: Weight,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopicWeight {
    pub topic: TopicId,
    pub weight: Weight,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Weights {
    /// One row per idea that can still take paint, in the manifest's order.
    pub ideas: Vec<IdeaWeight>,
    /// Each topic: the union of its ideas' text.
    pub topics: Vec<TopicWeight>,
    /// Text no idea covers.
    pub unpainted: Weight,
    /// The whole text (shares of 100%).
    pub total: Weight,
}

/// The weights of every idea in `old_text` (version `old_version`) against `new_text` (the
/// draft).
pub fn weights(
    manifest: &Manifest,
    old_version: u32,
    old_text: &str,
    old_paint: &[Span],
    new_text: &str,
    new_paint: &[Span],
) -> Weights {
    let old_starts = Starts::of(old_text);
    let new_starts = Starts::of(new_text);
    let old_total = old_starts.total();
    let new_total = new_starts.total();
    // How much of each text the spans of `ideas` cover, and whether that holds any words.
    let measure = |ideas: &[IdeaId]| {
        let old = covered(old_paint, |idea| ideas.contains(&idea));
        let new = covered(new_paint, |idea| ideas.contains(&idea));
        let weight = Weight {
            old: old_starts.counts_in(&old),
            new: new_starts.counts_in(&new),
            ..Weight::default()
        };
        (
            with_shares(weight, old_total, new_total),
            has_words(old_text, &old),
            has_words(new_text, &new),
        )
    };

    let ideas = manifest
        .live_ideas()
        .map(|idea| {
            let (weight, had_text, has_text) = measure(&manifest.lineage(idea.id));
            // Cut since the old version, or cut at some point and not in the old version either.
            let cut = idea.cut_in.iter().any(|&v| v > old_version || !had_text);
            let status = if has_text {
                Status::Carried
            } else if cut {
                Status::Cut
            } else if had_text {
                Status::Missing
            } else {
                Status::Unused
            };
            IdeaWeight {
                idea: idea.id,
                weight,
                status,
            }
        })
        .collect();

    let topics = manifest
        .topics
        .iter()
        .map(|topic| {
            let members: Vec<IdeaId> = manifest
                .live_ideas()
                .filter(|idea| idea.topic == Some(topic.id))
                .flat_map(|idea| manifest.lineage(idea.id))
                .collect();
            TopicWeight {
                topic: topic.id,
                weight: measure(&members).0,
            }
        })
        .collect();

    let old_gaps = gaps(&covered(old_paint, |_| true), old_text.len());
    let new_gaps = gaps(&covered(new_paint, |_| true), new_text.len());
    let unpainted = Weight {
        old: old_starts.counts_in(&old_gaps),
        new: new_starts.counts_in(&new_gaps),
        ..Weight::default()
    };
    let total = Weight {
        old: old_total,
        new: new_total,
        ..Weight::default()
    };
    Weights {
        ideas,
        topics,
        unpainted: with_shares(unpainted, old_total, new_total),
        total: with_shares(total, old_total, new_total),
    }
}

/// What the Commit dialog shows before a commit.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitCheck {
    /// The newest version's counts, and the draft's.
    pub old: Counts,
    pub new: Counts,
    pub carried: Vec<IdeaId>,
    pub cut: Vec<IdeaId>,
    /// Ideas with no text left: the dialog's "Heads up".
    pub missing: Vec<IdeaId>,
}

/// The Commit check: the draft (`new_*`) against the newest version (`old_*`).
pub fn commit_check(
    manifest: &Manifest,
    old_text: &str,
    old_paint: &[Span],
    new_text: &str,
    new_paint: &[Span],
) -> CommitCheck {
    let latest = manifest.latest_version();
    let weights = weights(manifest, latest, old_text, old_paint, new_text, new_paint);
    let with_status = |status: Status| {
        weights
            .ideas
            .iter()
            .filter(|row| row.status == status)
            .map(|row| row.idea)
            .collect()
    };
    CommitCheck {
        old: weights.total.old,
        new: weights.total.new,
        carried: with_status(Status::Carried),
        cut: with_status(Status::Cut),
        missing: with_status(Status::Missing),
    }
}

/// `part` as a percent of `whole`: by tokens when both are known, by words otherwise. 0 when the
/// whole is empty.
pub fn share(part: Counts, whole: Counts) -> f64 {
    let (part, whole) = match (part.tokens, whole.tokens) {
        (Some(part), Some(whole)) => (part, whole),
        _ => (part.words, whole.words),
    };
    match whole {
        0 => 0.0,
        whole => part as f64 / whole as f64 * 100.0,
    }
}

fn with_shares(weight: Weight, old_total: Counts, new_total: Counts) -> Weight {
    Weight {
        old_share: share(weight.old, old_total),
        new_share: share(weight.new, new_total),
        ..weight
    }
}

/// The text covered by the spans whose idea passes `include`, as sorted ranges that neither
/// overlap nor touch.
fn covered(paint: &[Span], include: impl Fn(IdeaId) -> bool) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = paint
        .iter()
        .filter(|span| include(span.idea))
        .map(|span| span.start..span.end)
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut joined: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match joined.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    joined
}

/// The parts of `0..len` that `ranges` (sorted, not overlapping) leave out.
fn gaps(ranges: &[Range<usize>], len: usize) -> Vec<Range<usize>> {
    let mut gaps = Vec::new();
    let mut from = 0;
    for range in ranges {
        if range.start > from {
            gaps.push(from..range.start);
        }
        from = from.max(range.end);
    }
    if from < len {
        gaps.push(from..len);
    }
    gaps
}

/// Whether the given stretches of `text` hold a letter or digit outside tags and {{macros}}. A
/// stray space or full stop left painted after a delete is not the idea's text.
fn has_words(text: &str, ranges: &[Range<usize>]) -> bool {
    ranges.iter().any(|range| {
        let piece = text.get(range.clone()).unwrap_or("");
        without_markup(piece).chars().any(char::is_alphanumeric)
    })
}

/// Where each word and each token of a text starts (tokens once the tokenizer is ready), so the
/// text is read once however many stretches of it are counted.
struct Starts {
    words: Vec<usize>,
    tokens: Option<Vec<usize>>,
}

impl Starts {
    fn of(text: &str) -> Starts {
        let words = text
            .char_indices()
            .filter(|&(at, c)| {
                let after_space = text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace);
                !c.is_whitespace() && after_space
            })
            .map(|(at, _)| at)
            .collect();
        Starts {
            words,
            tokens: token_starts(text),
        }
    }

    /// Every word and token in the text.
    fn total(&self) -> Counts {
        Counts {
            words: self.words.len(),
            tokens: self.tokens.as_ref().map(Vec::len),
        }
    }

    /// The words and tokens that start inside `ranges` (sorted, not overlapping).
    fn counts_in(&self, ranges: &[Range<usize>]) -> Counts {
        let inside = |starts: &[usize]| -> usize {
            ranges
                .iter()
                .map(|range| {
                    let before_end = starts.partition_point(|&at| at < range.end);
                    before_end - starts.partition_point(|&at| at < range.start)
                })
                .sum()
        };
        Counts {
            words: inside(&self.words),
            tokens: self.tokens.as_deref().map(inside),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::counts::load_tokenizer;

    fn span(idea: IdeaId, start: usize, end: usize) -> Span {
        Span {
            idea,
            start,
            end,
            quote: String::new(),
        }
    }

    /// A manifest at v1 (so the draft is version 2) with ideas 1, 2 and 3.
    fn three_ideas() -> Manifest {
        let mut manifest = Manifest::new("A");
        manifest.versions.push(crate::project::manifest::Version {
            number: 1,
            note: String::new(),
            committed_at: 0,
            paint: Vec::new(),
        });
        for name in ["one", "two", "three"] {
            manifest.add_idea(name);
        }
        manifest
    }

    fn row(weights: &Weights, idea: IdeaId) -> &IdeaWeight {
        weights.ideas.iter().find(|row| row.idea == idea).unwrap()
    }

    // Words start every 3 bytes: "aa" is 0..2, "bb" 3..5, "cc" 6..8 and so on.
    const OLD: &str = "aa bb cc dd ee ff gg hh ii jj";
    const NEW: &str = "aa bb cc dd";

    #[test]
    fn overlaps_count_once_per_idea_but_toward_every_idea() {
        load_tokenizer();
        let manifest = three_ideas();
        // Idea 1: two overlapping spans over "aa bb cc" (one word each would double count).
        // Idea 2: "bb cc dd", sharing "bb cc" with idea 1.
        let paint = vec![span(1, 0, 5), span(1, 3, 8), span(2, 3, 11)];
        let weights = weights(&manifest, 1, NEW, &paint, NEW, &paint);
        assert_eq!(row(&weights, 1).weight.new.words, 3);
        assert_eq!(row(&weights, 2).weight.new.words, 3);
        assert_eq!(weights.total.new.words, 4);
        assert_eq!(weights.unpainted.new.words, 0);
        let sum: f64 = weights.ideas.iter().map(|row| row.weight.new_share).sum();
        assert!(sum > 100.0, "shared text counts twice: {sum}");
        assert_eq!(weights.total.new_share, 100.0);
    }

    #[test]
    fn unpainted_is_what_no_idea_covers() {
        let manifest = three_ideas();
        let old_paint = vec![span(1, 0, 2), span(2, 12, 17)];
        let weights = weights(&manifest, 1, OLD, &old_paint, NEW, &[]);
        assert_eq!(weights.unpainted.old.words, 10 - 3);
        assert_eq!(weights.unpainted.new.words, 4);
        assert_eq!(weights.total.old.words, 10);
    }

    #[test]
    fn statuses_carried_cut_missing_and_unused() {
        let mut manifest = three_ideas();
        manifest.add_idea("four");
        let old_paint = vec![span(1, 0, 2), span(2, 3, 5), span(3, 6, 8)];
        let new_paint = vec![span(1, 0, 2)];
        manifest.set_cut(2, 2, true);
        manifest.set_cut(3, 1, true); // cut from an older version, not this draft
        let weights = weights(&manifest, 1, OLD, &old_paint, NEW, &new_paint);
        assert_eq!(row(&weights, 1).status, Status::Carried);
        assert_eq!(row(&weights, 2).status, Status::Cut);
        assert_eq!(row(&weights, 3).status, Status::Missing);
        assert_eq!(row(&weights, 4).status, Status::Unused);
        assert_eq!(row(&weights, 3).weight.old.words, 1);
        assert_eq!(row(&weights, 3).weight.new_share, 0.0);
    }

    #[test]
    fn a_full_stop_or_space_left_painted_is_not_text() {
        let manifest = three_ideas();
        let new = "aa. bb {{user}}";
        let old_paint = vec![span(1, 0, 2), span(2, 4, 6), span(3, 7, 15)];
        let new_paint = vec![span(1, 2, 4), span(2, 4, 6), span(3, 7, 15)];
        let weights = weights(&manifest, 1, OLD, &old_paint, new, &new_paint);
        assert_eq!(row(&weights, 1).status, Status::Missing, "only \". \" left");
        assert_eq!(row(&weights, 2).status, Status::Carried);
        assert_eq!(
            row(&weights, 3).status,
            Status::Missing,
            "only a macro left"
        );
    }

    #[test]
    fn a_text_split_between_ideas_adds_up_to_exactly_100_percent() {
        load_tokenizer();
        let manifest = three_ideas();
        let text = "Nyx is quiet. She keeps her distance.\n\nShe never explains herself.";
        // Each sentence its own idea; the spaces and the blank line between are unpainted.
        let paint = vec![span(1, 0, 13), span(2, 14, 37), span(3, 39, 66)];
        let split = weights(&manifest, 1, text, &paint, text, &paint);
        let ideas: f64 = split.ideas.iter().map(|row| row.weight.new_share).sum();
        let sum = ideas + split.unpainted.new_share;
        assert!((sum - 100.0).abs() < 1e-9, "got {sum}");
        assert_eq!(split.unpainted.new.words, 0);
        assert_eq!(split.unpainted.new.tokens, Some(0));
        // Painting the start of a word and its end apart still counts the word once.
        let paint = vec![span(1, 0, 1), span(1, 2, 3)];
        let word = weights(&manifest, 1, "Nyx", &paint, "Nyx", &paint);
        assert_eq!(row(&word, 1).weight.new.words, 1);
        assert_eq!(word.unpainted.new.words, 0);
    }

    #[test]
    fn a_cut_mark_carries_on_into_later_drafts() {
        let mut manifest = three_ideas();
        manifest.versions.push(crate::project::manifest::Version {
            number: 2,
            note: String::new(),
            committed_at: 0,
            paint: Vec::new(),
        });
        // Idea 1 had text in v1 and was cut on purpose when v2 was committed; the draft is v3.
        manifest.set_cut(1, 2, true);
        let old_paint = vec![span(1, 0, 2)];
        let from_v1 = weights(&manifest, 1, OLD, &old_paint, NEW, &[]);
        assert_eq!(row(&from_v1, 1).status, Status::Cut, "not missing again");
        let from_v2 = weights(&manifest, 2, NEW, &[], NEW, &[]);
        assert_eq!(row(&from_v2, 1).status, Status::Cut);
        assert_eq!(row(&from_v2, 2).status, Status::Unused);
    }

    #[test]
    fn merged_ideas_count_toward_the_idea_they_went_into() {
        let mut manifest = three_ideas();
        let old_paint = vec![span(1, 0, 2), span(2, 3, 5)];
        let mut new_paint = vec![span(1, 0, 2), span(2, 3, 5)];
        let merged = manifest
            .merge_ideas(1, 2, "both", "", &mut new_paint)
            .unwrap();
        let weights = weights(&manifest, 1, OLD, &old_paint, NEW, &new_paint);
        let rows: Vec<IdeaId> = weights.ideas.iter().map(|row| row.idea).collect();
        assert_eq!(rows, [3, merged], "the merged pair has no rows of its own");
        assert_eq!(row(&weights, merged).weight.old.words, 2);
        assert_eq!(row(&weights, merged).weight.new.words, 2);
        assert_eq!(row(&weights, merged).status, Status::Carried);
    }

    #[test]
    fn topics_roll_up_their_ideas_without_double_counting() {
        let mut manifest = three_ideas();
        let voice = manifest.add_topic("Voice");
        let empty = manifest.add_topic("Empty");
        manifest.set_topic(1, Some(voice));
        manifest.set_topic(2, Some(voice));
        let paint = vec![span(1, 0, 5), span(2, 3, 8), span(3, 9, 11)];
        let weights = weights(&manifest, 1, NEW, &paint, NEW, &paint);
        assert_eq!(weights.topics[0].topic, voice);
        assert_eq!(weights.topics[0].weight.new.words, 3);
        assert_eq!(weights.topics[1].topic, empty);
        assert_eq!(weights.topics[1].weight.old.words, 0);
        assert_eq!(weights.topics[1].weight.new_share, 0.0);
    }

    #[test]
    fn share_falls_back_to_words_until_tokens_are_known() {
        let part = Counts {
            words: 1,
            tokens: Some(3),
        };
        let whole = Counts {
            words: 4,
            tokens: Some(6),
        };
        assert_eq!(share(part, whole), 50.0);
        let loading = Counts {
            words: 4,
            tokens: None,
        };
        assert_eq!(share(part, loading), 25.0);
        assert_eq!(share(part, Counts::default()), 0.0);
    }

    #[test]
    fn commit_check_sorts_ideas_and_reports_counts() {
        let mut manifest = three_ideas();
        manifest.set_cut(2, 2, true);
        let old_paint = vec![span(1, 0, 2), span(2, 3, 5), span(3, 6, 8)];
        let new_paint = vec![span(1, 0, 2)];
        let check = commit_check(&manifest, OLD, &old_paint, NEW, &new_paint);
        assert_eq!(check.old.words, 10);
        assert_eq!(check.new.words, 4);
        assert_eq!(check.carried, [1]);
        assert_eq!(check.cut, [2]);
        assert_eq!(check.missing, [3]);
    }

    #[test]
    fn covered_and_gaps_join_and_complement_ranges() {
        let paint = vec![span(1, 5, 8), span(2, 0, 2), span(1, 2, 3), span(1, 7, 10)];
        assert_eq!(covered(&paint, |_| true), [0..3, 5..10]);
        assert_eq!(covered(&paint, |idea| idea == 1), [2..3, 5..10]);
        assert!(covered(&paint, |_| false).is_empty());
        assert_eq!(gaps(&[0..3, 5..10], 12), [3..5, 10..12]);
        assert_eq!(gaps(&[], 4), vec![0..4; 1]);
        assert!(gaps(&[0..2, 2..4], 4).is_empty());
    }
}
