//! Ideas and topics: the named concepts the writer paints onto the text, kept in the manifest.
//!
//! Topics hold ideas (two levels). Each idea has a colour, a feeling note per version and, when it
//! was cut from a version on purpose, a mark saying so. Merging and splitting record where an idea
//! came from, like a git merge.
//!
//! Functions that change paint take the draft's paint as `draft_paint`, because while a window is
//! open the live copy is the editor's (`TextModel::paint`), not `manifest.draft.paint`.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::core::markup::without_markup;
use crate::project::manifest::Manifest;
use crate::project::paint::{self, IdeaId, Span};

/// Which topic an idea belongs to. A plain number, stored in the manifest.
pub type TopicId = u32;

/// How many idea colours there are. The UI maps each slot to a palette role, in this order: aqua,
/// blue, purple, yellow, orange, green, subtle grey. Never red.
pub const COLOUR_SLOTS: usize = 7;

/// How many words of a selection name an idea made from it, and the longest such name.
const NAME_WORDS: usize = 3;
const NAME_MAX_CHARS: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Idea {
    pub id: IdeaId,
    pub name: String,
    /// A slot in the colour rotation, `0..COLOUR_SLOTS`.
    pub colour: usize,
    #[serde(default)]
    pub topic: Option<TopicId>,
    /// Feeling notes, each filed under the version it was written for. The draft's note is filed
    /// under the number the draft will get. Older entries stay, so history keeps every note.
    #[serde(default)]
    pub notes: Vec<Note>,
    /// Versions this idea was cut from on purpose, so having no text there isn't "missing".
    #[serde(default)]
    pub cut_in: Vec<u32>,
    /// The ideas this one was made from by a merge (two) or a split (one).
    #[serde(default)]
    pub parents: Vec<IdeaId>,
    /// Set once this idea was merged into another. It then has no row of its own: its paint in
    /// older versions counts toward the idea it went into.
    #[serde(default)]
    pub merged_into: Option<IdeaId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub version: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub id: TopicId,
    pub name: String,
}

impl Manifest {
    pub fn idea(&self, id: IdeaId) -> Option<&Idea> {
        self.ideas.iter().find(|idea| idea.id == id)
    }

    fn idea_mut(&mut self, id: IdeaId) -> Option<&mut Idea> {
        self.ideas.iter_mut().find(|idea| idea.id == id)
    }

    /// Ideas that can still take paint (not merged into another), in the order they were made.
    pub fn live_ideas(&self) -> impl Iterator<Item = &Idea> {
        self.ideas.iter().filter(|idea| idea.merged_into.is_none())
    }

    /// Adds an idea with the least used colour and returns its id.
    pub fn add_idea(&mut self, name: &str) -> IdeaId {
        let id = self.ideas.iter().map(|idea| idea.id).max().unwrap_or(0) + 1;
        let colour = (0..COLOUR_SLOTS)
            .min_by_key(|&slot| self.live_ideas().filter(|i| i.colour == slot).count())
            .unwrap_or(0);
        self.ideas.push(Idea {
            id,
            name: name.trim().to_string(),
            colour,
            topic: None,
            notes: Vec::new(),
            cut_in: Vec::new(),
            parents: Vec::new(),
            merged_into: None,
        });
        id
    }

    /// Makes an idea from selected text: named after its first few words (skipping XML tags and
    /// {{macros}}), and painted over it.
    pub fn idea_from_selection(
        &mut self,
        text: &str,
        range: Range<usize>,
        draft_paint: &mut Vec<Span>,
    ) -> IdeaId {
        let selected = without_markup(text.get(range.clone()).unwrap_or(""));
        let name: String = selected
            .split_whitespace()
            .take(NAME_WORDS)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(NAME_MAX_CHARS)
            .collect();
        let id = match name.is_empty() {
            true => self.add_idea("New idea"),
            false => self.add_idea(&name),
        };
        paint::brush(draft_paint, range, id);
        id
    }

    pub fn rename_idea(&mut self, id: IdeaId, name: &str) {
        if let Some(idea) = self.idea_mut(id) {
            idea.name = name.trim().to_string();
        }
    }

    pub fn recolour_idea(&mut self, id: IdeaId, colour: usize) {
        if let Some(idea) = self.idea_mut(id) {
            idea.colour = colour % COLOUR_SLOTS;
        }
    }

    /// Puts an idea under a topic, or under none.
    pub fn set_topic(&mut self, id: IdeaId, topic: Option<TopicId>) {
        if let Some(idea) = self.idea_mut(id) {
            idea.topic = topic;
        }
    }

    /// Deletes an idea and its paint everywhere: the draft and every version. Deleting means it
    /// was never an idea, so nothing is left pointing at it. Ideas that had been merged into it
    /// get their own rows back.
    pub fn delete_idea(&mut self, id: IdeaId, draft_paint: &mut Vec<Span>) {
        self.ideas.retain(|idea| idea.id != id);
        for idea in &mut self.ideas {
            if idea.merged_into == Some(id) {
                idea.merged_into = None;
            }
        }
        draft_paint.retain(|span| span.idea != id);
        self.draft.paint.retain(|span| span.idea != id);
        for version in &mut self.versions {
            version.paint.retain(|span| span.idea != id);
        }
    }

    /// The feeling note for `version`: the newest one written at or before it, "" if none.
    pub fn note(&self, id: IdeaId, version: u32) -> &str {
        self.idea(id)
            .and_then(|idea| idea.notes.iter().rfind(|note| note.version <= version))
            .map_or("", |note| note.text.as_str())
    }

    /// Writes the feeling note for `version` (usually `draft_version()`), replacing any note
    /// already written for that same version. Notes of other versions are kept.
    pub fn set_note(&mut self, id: IdeaId, version: u32, text: &str) {
        let Some(idea) = self.idea_mut(id) else {
            return;
        };
        idea.notes.retain(|note| note.version != version);
        let at = idea.notes.partition_point(|note| note.version < version);
        let note = Note {
            version,
            text: text.trim().to_string(),
        };
        idea.notes.insert(at, note);
    }

    /// The newest version's feeling note and its number, when the draft's note says something
    /// else: what the note card shows as `v3 said "…"`. `None` while they agree, or the newest
    /// version had no note.
    pub fn previous_note(&self, id: IdeaId) -> Option<(u32, &str)> {
        let latest = self.latest_version();
        let before = self.note(id, latest);
        let changed = !before.is_empty() && before != self.note(id, self.draft_version());
        changed.then_some((latest, before))
    }

    /// Marks an idea as cut on purpose from `version`, or takes every cut mark off it (an older
    /// mark still quiets the missing flag; see `weights`).
    pub fn set_cut(&mut self, id: IdeaId, version: u32, cut: bool) {
        if let Some(idea) = self.idea_mut(id) {
            match cut {
                true if !idea.cut_in.contains(&version) => idea.cut_in.push(version),
                true => {}
                false => idea.cut_in.clear(),
            }
        }
    }

    pub fn add_topic(&mut self, name: &str) -> TopicId {
        let id = self.topics.iter().map(|topic| topic.id).max().unwrap_or(0) + 1;
        self.topics.push(Topic {
            id,
            name: name.trim().to_string(),
        });
        id
    }

    pub fn rename_topic(&mut self, id: TopicId, name: &str) {
        if let Some(topic) = self.topics.iter_mut().find(|topic| topic.id == id) {
            topic.name = name.trim().to_string();
        }
    }

    /// Deletes a topic. Its ideas stay, under no topic.
    pub fn delete_topic(&mut self, id: TopicId) {
        self.topics.retain(|topic| topic.id != id);
        for idea in &mut self.ideas {
            if idea.topic == Some(id) {
                idea.topic = None;
            }
        }
    }

    /// Merges ideas `a` and `b` into a new idea with its own name and note. The draft's paint of
    /// both becomes the new idea's; older versions keep theirs, and count toward the new idea.
    pub fn merge_ideas(
        &mut self,
        a: IdeaId,
        b: IdeaId,
        name: &str,
        note: &str,
        draft_paint: &mut Vec<Span>,
    ) -> Option<IdeaId> {
        let (colour, topic) = {
            let first = self.idea(a)?;
            let second = self.idea(b)?;
            (first.colour, first.topic.or(second.topic))
        };
        if a == b {
            return None;
        }
        let merged = self.add_idea(name);
        let version = self.draft_version();
        self.set_note(merged, version, note);
        if let Some(idea) = self.idea_mut(merged) {
            idea.colour = colour;
            idea.topic = topic;
            idea.parents = vec![a, b];
        }
        for parent in [a, b] {
            if let Some(idea) = self.idea_mut(parent) {
                idea.merged_into = Some(merged);
            }
            paint::relabel(draft_paint, parent, merged);
        }
        Some(merged)
    }

    /// Splits off part of an idea: a new idea (same topic and note, its own colour) takes over
    /// `id`'s draft paint inside `range`. The rest stays with `id`. Repaint to move more.
    pub fn split_idea(
        &mut self,
        id: IdeaId,
        range: Range<usize>,
        name: &str,
        draft_paint: &mut Vec<Span>,
    ) -> Option<IdeaId> {
        let topic = self.idea(id)?.topic;
        let version = self.draft_version();
        let note = self.note(id, version).to_string();
        let split = self.add_idea(name);
        self.set_note(split, version, &note);
        if let Some(idea) = self.idea_mut(split) {
            idea.topic = topic;
            idea.parents = vec![id];
        }
        let taken: Vec<Range<usize>> = draft_paint
            .iter()
            .filter(|span| span.idea == id)
            .map(|span| span.start.max(range.start)..span.end.min(range.end))
            .filter(|piece| !piece.is_empty())
            .collect();
        paint::unpaint(draft_paint, range, id);
        for piece in taken {
            paint::brush(draft_paint, piece, split);
        }
        Some(split)
    }

    /// `id` and every idea merged into it, directly or through other merges.
    pub fn lineage(&self, id: IdeaId) -> Vec<IdeaId> {
        self.ideas
            .iter()
            .filter(|idea| {
                let mut current = idea.id;
                // At most one step per idea, so a broken file with a merge loop can't hang us.
                for _ in 0..=self.ideas.len() {
                    if current == id {
                        return true;
                    }
                    match self.idea(current).and_then(|idea| idea.merged_into) {
                        Some(next) => current = next,
                        None => return false,
                    }
                }
                false
            })
            .map(|idea| idea.id)
            .collect()
    }
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

    #[test]
    fn add_rename_recolour_and_topics() {
        let mut manifest = Manifest::new("A");
        let autonomy = manifest.add_idea(" Autonomy ");
        let slow = manifest.add_idea("Slow burn");
        assert_eq!((autonomy, slow), (1, 2));
        assert_eq!(manifest.idea(autonomy).unwrap().name, "Autonomy");
        assert_eq!(
            manifest.idea(slow).unwrap().colour,
            1,
            "the next free colour"
        );

        manifest.rename_idea(slow, "Slower burn");
        manifest.recolour_idea(slow, COLOUR_SLOTS + 3);
        assert_eq!(manifest.idea(slow).unwrap().name, "Slower burn");
        assert_eq!(manifest.idea(slow).unwrap().colour, 3);

        let rules = manifest.add_topic("Dialogue rules");
        manifest.set_topic(autonomy, Some(rules));
        manifest.rename_topic(rules, "Rules");
        assert_eq!(manifest.topics[0].name, "Rules");
        assert_eq!(manifest.idea(autonomy).unwrap().topic, Some(rules));
        manifest.delete_topic(rules);
        assert!(manifest.topics.is_empty());
        assert_eq!(manifest.idea(autonomy).unwrap().topic, None);
    }

    #[test]
    fn colours_go_round_and_reuse_freed_slots() {
        let mut manifest = Manifest::new("A");
        for n in 0..COLOUR_SLOTS + 1 {
            manifest.add_idea(&n.to_string());
        }
        assert_eq!(manifest.ideas[COLOUR_SLOTS].colour, 0, "wrapped round");
        let mut paint = Vec::new();
        manifest.delete_idea(3, &mut paint); // had colour 2
        let reused = manifest.add_idea("x");
        assert_eq!(manifest.idea(reused).unwrap().colour, 2);
    }

    #[test]
    fn delete_takes_its_paint_out_of_every_version() {
        let mut manifest = Manifest::new("A");
        manifest.versions.push(crate::project::manifest::Version {
            number: 1,
            note: String::new(),
            committed_at: 0,
            paint: vec![span(1, 0, 3), span(2, 0, 3)],
        });
        manifest.add_idea("one");
        manifest.add_idea("two");
        manifest.draft.paint = vec![span(1, 0, 3)];
        let mut draft = vec![span(1, 0, 3), span(2, 4, 6)];
        manifest.delete_idea(1, &mut draft);
        assert_eq!(draft, vec![span(2, 4, 6)]);
        assert!(manifest.draft.paint.is_empty());
        assert_eq!(manifest.versions[0].paint, vec![span(2, 0, 3)]);
        assert!(manifest.idea(1).is_none());
        // The next new idea doesn't pick up the deleted one's old paint by reusing its id.
        assert_eq!(manifest.add_idea("three"), 3);
    }

    #[test]
    fn notes_are_kept_per_version() {
        let mut manifest = Manifest::new("A");
        let id = manifest.add_idea("Nyx");
        assert_eq!(manifest.note(id, 1), "");
        manifest.set_note(id, 1, "warm but guarded");
        manifest.set_note(id, 3, "colder");
        assert_eq!(manifest.note(id, 1), "warm but guarded");
        assert_eq!(
            manifest.note(id, 2),
            "warm but guarded",
            "carried until changed"
        );
        assert_eq!(manifest.note(id, 3), "colder");
        manifest.set_note(id, 2, "in between");
        manifest.set_note(id, 3, "cold");
        let versions: Vec<u32> = manifest.ideas[0].notes.iter().map(|n| n.version).collect();
        assert_eq!(versions, [1, 2, 3], "one note per version, in order");
        assert_eq!(manifest.note(id, 9), "cold");
    }

    #[test]
    fn previous_note_shows_only_once_the_draft_changes_it() {
        let mut manifest = Manifest::new("A");
        manifest.versions.push(crate::project::manifest::Version {
            number: 1,
            note: String::new(),
            committed_at: 0,
            paint: Vec::new(),
        });
        let id = manifest.add_idea("Prose");
        manifest.set_note(id, 2, "fresh");
        assert_eq!(manifest.previous_note(id), None, "v1 had no note");
        manifest.set_note(id, 1, "fresh");
        assert_eq!(manifest.previous_note(id), None, "the same words");
        manifest.set_note(id, 2, "vivid but restrained");
        assert_eq!(manifest.previous_note(id), Some((1, "fresh")));
    }

    #[test]
    fn cut_on_purpose_is_per_version() {
        let mut manifest = Manifest::new("A");
        let id = manifest.add_idea("Nyx");
        manifest.set_cut(id, 2, true);
        manifest.set_cut(id, 2, true);
        manifest.set_cut(id, 3, true);
        assert_eq!(manifest.ideas[0].cut_in, [2, 3]);
        // Taking it off takes every mark off, older ones too.
        manifest.set_cut(id, 3, false);
        assert!(manifest.ideas[0].cut_in.is_empty());
    }

    #[test]
    fn idea_from_selection_names_and_paints_it() {
        let mut manifest = Manifest::new("A");
        let text = "She keeps   her distance at first, always.";
        let mut paint = Vec::new();
        let id = manifest.idea_from_selection(text, 0..33, &mut paint);
        assert_eq!(manifest.idea(id).unwrap().name, "She keeps her");
        assert_eq!(paint, vec![span(id, 0, 33)]);
        let blank = manifest.idea_from_selection(text, 3..4, &mut paint);
        assert_eq!(manifest.idea(blank).unwrap().name, "New idea");
        let tagged = "<rules>{{char}} never lies</rules>";
        let id = manifest.idea_from_selection(tagged, 0..tagged.len(), &mut paint);
        assert_eq!(manifest.idea(id).unwrap().name, "never lies");
    }

    #[test]
    fn merge_makes_a_new_idea_and_records_its_parents() {
        let mut manifest = Manifest::new("A");
        manifest.versions.push(crate::project::manifest::Version {
            number: 1,
            note: String::new(),
            committed_at: 0,
            paint: vec![span(1, 0, 3), span(2, 5, 8)],
        });
        let a = manifest.add_idea("Warm");
        let b = manifest.add_idea("Guarded");
        let topic = manifest.add_topic("Voice");
        manifest.set_topic(b, Some(topic));
        let mut draft = vec![span(a, 0, 4), span(b, 4, 9), span(9, 0, 2)];

        assert_eq!(manifest.merge_ideas(a, a, "x", "", &mut draft), None);
        assert_eq!(manifest.merge_ideas(a, 99, "x", "", &mut draft), None);
        let merged = manifest
            .merge_ideas(a, b, "Warm but guarded", "both at once", &mut draft)
            .unwrap();
        let idea = manifest.idea(merged).unwrap();
        assert_eq!(idea.parents, [a, b]);
        assert_eq!(idea.topic, Some(topic));
        assert_eq!(idea.colour, manifest.idea(a).unwrap().colour);
        assert_eq!(manifest.note(merged, 2), "both at once");
        assert_eq!(draft, vec![span(9, 0, 2), span(merged, 0, 9)]);
        assert_eq!(manifest.idea(a).unwrap().merged_into, Some(merged));
        let live: Vec<IdeaId> = manifest.live_ideas().map(|idea| idea.id).collect();
        assert_eq!(live, [merged]);
        assert_eq!(manifest.lineage(merged), [a, b, merged]);
        // The old version's paint is untouched.
        assert_eq!(
            manifest.versions[0].paint,
            vec![span(1, 0, 3), span(2, 5, 8)]
        );
        // Merging the merge again chains the lineage.
        let other = manifest.add_idea("Other");
        let again = manifest
            .merge_ideas(merged, other, "All", "", &mut draft)
            .unwrap();
        assert_eq!(manifest.lineage(again), [a, b, merged, other, again]);
    }

    #[test]
    fn split_takes_part_of_the_paint() {
        let mut manifest = Manifest::new("A");
        let id = manifest.add_idea("Voice");
        manifest.set_note(id, 1, "warm");
        let mut draft = vec![span(id, 0, 10), span(id, 20, 30)];
        let split = manifest.split_idea(id, 5..25, "Tone", &mut draft).unwrap();
        assert_eq!(
            draft,
            vec![
                span(id, 0, 5),
                span(id, 25, 30),
                span(split, 5, 10),
                span(split, 20, 25)
            ]
        );
        let idea = manifest.idea(split).unwrap();
        assert_eq!(idea.parents, [id]);
        assert_ne!(idea.colour, manifest.idea(id).unwrap().colour);
        assert_eq!(manifest.note(split, 1), "warm");
        assert_eq!(manifest.split_idea(99, 0..1, "x", &mut draft), None);
    }

    #[test]
    fn a_merge_loop_in_a_broken_file_does_not_hang() {
        let mut manifest = Manifest::new("A");
        let a = manifest.add_idea("a");
        let b = manifest.add_idea("b");
        manifest.ideas[0].merged_into = Some(b);
        manifest.ideas[1].merged_into = Some(a);
        assert_eq!(manifest.lineage(a), [a, b]);
    }

    #[test]
    fn ideas_round_trip_through_json() {
        let mut manifest = Manifest::new("A");
        let id = manifest.add_idea("Nyx");
        manifest.set_note(id, 1, "warm");
        manifest.set_cut(id, 2, true);
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), manifest);
        // Only id, name and colour are required.
        let idea: Idea = serde_json::from_str(r#"{"id":4,"name":"x","colour":1}"#).unwrap();
        assert!(idea.notes.is_empty() && idea.topic.is_none() && idea.merged_into.is_none());
    }
}
