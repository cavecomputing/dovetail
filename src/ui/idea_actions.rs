//! The ideas half of `App::update`: brushes, painting and erasing selections in either pane,
//! making, editing, merging, splitting and deleting ideas, and the weights view.

use std::ops::Range;

use iced::widget::operation::{focus, scroll_by};
use iced::widget::scrollable::AbsoluteOffset;
use iced::{Rectangle, Task, mouse};

use crate::project::ideas::TopicId;
use crate::project::paint::{self, IdeaId, Span};
use crate::project::weights::Status;
use crate::ui::app::{App, Message};
use crate::ui::commit_dialog::COMMIT_NOTE_ID;
use crate::ui::editor;
use crate::ui::idea_panel::{IDEA_NAME_ID, IdeaPanel, MERGE_NAME_ID, MergeForm, NOTE_ID};
use crate::ui::ideas_view::STRIP_ID;
use crate::ui::weights_view::Measure;

#[derive(Debug, Clone)]
pub enum IdeaAction {
    /// A chip was clicked: paint the draft's selection with that idea and pick it as the brush,
    /// or with nothing selected pick it or put it down.
    ToggleBrush(IdeaId),
    /// "+ New idea".
    New,
    /// Ctrl+I: an idea made from the selected text, painted over it.
    FromSelection,
    /// Ctrl+E (`None`: the brush's paint, or all paint without a brush) or the panel (that
    /// idea's): takes paint off the selected text.
    Erase(Option<IdeaId>),
    /// The mouse button went up: with a brush, paint what the mouse just selected in the left
    /// pane.
    PaintSelection,
    OpenPanel(IdeaId),
    /// The panel's "Feeling note…", or a click on the note card: the panel with its note editor.
    OpenNote(IdeaId),
    Name(String),
    Note(String),
    Colour(usize),
    Topic(Option<TopicId>),
    TopicName(String),
    AddTopic,
    /// Renames the panel's idea's topic to what the new-topic field holds.
    RenameTopic,
    DeleteTopic(TopicId),
    ToggleCut(IdeaId),
    /// "Merge with…" picked this idea: the panel asks for the merged idea's name and note.
    MergeWith(IdeaId),
    MergeName(String),
    MergeNote(String),
    /// Merges the two ideas, as the merge form says.
    Merge,
    /// Splits the selected part of the panel's idea off into a new idea.
    Split,
    Delete,
    /// Where the draft's caret is drawn, and the text area it is in.
    Caret(Option<Rectangle>, Rectangle),
    ToggleWeights,
    Measure(Measure),
    /// The mouse wheel over the idea strip, which scrolls it sideways.
    StripScroll(mouse::ScrollDelta),
    /// The Commit dialog's "mark cut": every missing idea is cut on purpose.
    CutMissing,
}

impl App {
    /// Handles the idea messages; `App::update` hands every one of them here.
    pub fn update_ideas(&mut self, action: IdeaAction) -> Task<Message> {
        let editable = self.editable();
        let Some(project) = &mut self.project else {
            return Task::none();
        };
        let draft = &mut self.pane;
        let manifest = &mut project.project.manifest;
        let version = manifest.draft_version();
        let panel_idea = project.idea_panel.as_ref().map(|panel| panel.idea);
        match action {
            IdeaAction::Caret(caret, text_area) => {
                project.caret = caret;
                project.text_area = text_area;
                return Task::none();
            }
            IdeaAction::ToggleWeights => {
                self.menu_open = false;
                project.weights_open = !project.weights_open;
                return Task::none();
            }
            IdeaAction::Measure(measure) => {
                project.measure = measure;
                return Task::none();
            }
            IdeaAction::StripScroll(delta) => {
                let pixels = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * 60.0,
                    mouse::ScrollDelta::Pixels { y, .. } => y,
                };
                return scroll_by(STRIP_ID, AbsoluteOffset { x: -pixels, y: 0.0 });
            }
            _ if !editable => return Task::none(),
            IdeaAction::ToggleBrush(id) => {
                // Nothing is ever painted from a selection the left pane no longer shows.
                project.reference.deselect();
                if draft.selection().is_some() {
                    draft.model.brush = Some(id);
                    self.change_selected_paint(|paint, range| paint::brush(paint, range, id));
                } else {
                    draft.model.brush = (draft.model.brush != Some(id)).then_some(id);
                }
                return focus(editor::EDITOR_ID);
            }
            IdeaAction::New => {
                let id = manifest.add_idea("New idea");
                draft.model.brush = Some(id);
                project.idea_panel = Some(IdeaPanel::new(id, "New idea", ""));
                self.ideas_changed();
                return focus(IDEA_NAME_ID);
            }
            IdeaAction::FromSelection => {
                let selected = match (draft.selection(), project.reference.selection()) {
                    (Some(range), _) => Some((draft.model.text(), range)),
                    (None, Some(range)) => Some((project.reference.model.text(), range)),
                    (None, None) => None,
                };
                let Some((text, range)) = selected else {
                    self.notice = Some("Select some text to make an idea from".to_string());
                    return Task::none();
                };
                // Named from the selection; the paint comes next, in whichever pane it is.
                let id = manifest.idea_from_selection(text, range, &mut Vec::new());
                draft.model.brush = Some(id);
                project.idea_panel = None;
                self.change_selected_paint(|paint, range| paint::brush(paint, range, id));
            }
            IdeaAction::Erase(idea) => {
                let ideas: Vec<IdeaId> = match idea.or(draft.model.brush) {
                    Some(id) => vec![id],
                    None => manifest.ideas.iter().map(|idea| idea.id).collect(),
                };
                self.change_selected_paint(|paint, range| {
                    for &id in &ideas {
                        paint::unpaint(paint, range.clone(), id);
                    }
                });
            }
            IdeaAction::PaintSelection => {
                // Only the release that ends a mouse selection in the left pane paints (selecting
                // there cleared the draft's selection). A selection in the draft stays one, to
                // delete, copy or retype.
                let selected = std::mem::take(&mut project.mouse_selecting);
                if let (true, Some(id)) = (selected, draft.model.brush) {
                    self.change_selected_paint(|paint, range| paint::brush(paint, range, id));
                }
            }
            IdeaAction::OpenPanel(id) | IdeaAction::OpenNote(id) => {
                let note = manifest.note(id, version);
                project.idea_panel = manifest
                    .idea(id)
                    .map(|idea| IdeaPanel::new(id, &idea.name, note));
                if matches!(action, IdeaAction::OpenNote(_))
                    && let Some(panel) = &mut project.idea_panel
                {
                    panel.editing_note = true;
                    return focus(NOTE_ID);
                }
            }
            IdeaAction::Name(name) => {
                if let Some(panel) = &mut project.idea_panel {
                    manifest.rename_idea(panel.idea, &name);
                    panel.name = name;
                    self.ideas_changed();
                }
            }
            IdeaAction::Note(note) => {
                if let Some(panel) = &mut project.idea_panel {
                    manifest.set_note(panel.idea, version, &note);
                    panel.note = note;
                    self.ideas_changed();
                }
            }
            IdeaAction::Colour(colour) => {
                if let Some(id) = panel_idea {
                    manifest.recolour_idea(id, colour);
                    self.ideas_changed();
                }
            }
            IdeaAction::Topic(topic) => {
                if let Some(id) = panel_idea {
                    manifest.set_topic(id, topic);
                    self.ideas_changed();
                }
            }
            IdeaAction::TopicName(name) => {
                if let Some(panel) = &mut project.idea_panel {
                    panel.new_topic = name;
                }
            }
            IdeaAction::AddTopic => {
                if let Some(panel) = &mut project.idea_panel
                    && !panel.new_topic.trim().is_empty()
                {
                    let topic = manifest.add_topic(&panel.new_topic);
                    manifest.set_topic(panel.idea, Some(topic));
                    panel.new_topic.clear();
                    self.ideas_changed();
                }
            }
            IdeaAction::RenameTopic => {
                if let Some(panel) = &mut project.idea_panel
                    && let Some(topic) = manifest.idea(panel.idea).and_then(|idea| idea.topic)
                    && !panel.new_topic.trim().is_empty()
                {
                    manifest.rename_topic(topic, &panel.new_topic);
                    panel.new_topic.clear();
                    self.ideas_changed();
                }
            }
            IdeaAction::DeleteTopic(topic) => {
                manifest.delete_topic(topic);
                self.ideas_changed();
            }
            IdeaAction::ToggleCut(id) => {
                // Undo takes off every cut mark, as an older one can be what makes it cut.
                let rows = &project.weights.ideas;
                let cut = rows
                    .iter()
                    .any(|row| row.idea == id && row.status == Status::Cut);
                manifest.set_cut(id, version, !cut);
                self.ideas_changed();
            }
            IdeaAction::CutMissing => {
                if let Some(check) = &project.commit_check {
                    for &id in &check.missing {
                        manifest.set_cut(id, version, true);
                    }
                }
                project.commit_check = project.check_commit(&draft.model);
                self.ideas_changed();
                // The button that was clicked is gone; the note can still be typed.
                return focus(COMMIT_NOTE_ID);
            }
            IdeaAction::MergeWith(other) => {
                let (Some(panel), Some(second)) = (&mut project.idea_panel, manifest.idea(other))
                else {
                    return Task::none();
                };
                let notes = [panel.note.trim(), manifest.note(other, version)];
                let notes: Vec<&str> = notes.into_iter().filter(|note| !note.is_empty()).collect();
                panel.merge = Some(MergeForm {
                    other,
                    name: format!("{} + {}", panel.name.trim(), second.name),
                    note: notes.join("; "),
                });
                return focus(MERGE_NAME_ID);
            }
            IdeaAction::MergeName(name) => {
                if let Some(form) = project.idea_panel.as_mut().and_then(|p| p.merge.as_mut()) {
                    form.name = name;
                }
            }
            IdeaAction::MergeNote(note) => {
                if let Some(form) = project.idea_panel.as_mut().and_then(|p| p.merge.as_mut()) {
                    form.note = note;
                }
            }
            IdeaAction::Merge => {
                let Some(panel) = &project.idea_panel else {
                    return Task::none();
                };
                let (id, Some(form)) = (panel.idea, &panel.merge) else {
                    return Task::none();
                };
                let (other, name, note) = (form.other, form.name.clone(), form.note.clone());
                let mut paint = draft.model.paint.clone();
                if let Some(merged) = manifest.merge_ideas(id, other, &name, &note, &mut paint) {
                    draft.model.set_paint(paint);
                    if draft
                        .model
                        .brush
                        .is_some_and(|brush| brush == id || brush == other)
                    {
                        draft.model.brush = Some(merged);
                    }
                    project.idea_panel = Some(IdeaPanel::new(merged, &name, &note));
                    self.ideas_changed();
                }
            }
            IdeaAction::Split => {
                let (Some(id), Some(range)) = (panel_idea, draft.selection()) else {
                    self.notice = Some("Select the draft text to split off first".to_string());
                    return Task::none();
                };
                let name = match manifest.idea(id) {
                    Some(idea) => format!("{} (split)", idea.name),
                    None => return Task::none(),
                };
                let mut paint = draft.model.paint.clone();
                if let Some(split) = manifest.split_idea(id, range, &name, &mut paint) {
                    draft.model.set_paint(paint);
                    draft.deselect();
                    let note = manifest.note(split, version);
                    project.idea_panel = Some(IdeaPanel::new(split, &name, note));
                    self.ideas_changed();
                }
            }
            IdeaAction::Delete => {
                let Some(panel) = &mut project.idea_panel else {
                    return Task::none();
                };
                if !panel.confirm_delete {
                    panel.confirm_delete = true;
                    return Task::none();
                }
                let id = panel.idea;
                project.idea_panel = None;
                manifest.delete_idea(id, &mut draft.model.paint);
                draft.model.forget_idea(id);
                if draft.model.brush == Some(id) {
                    draft.model.brush = None;
                }
                let reference = &mut project.reference;
                reference.model.paint = project
                    .project
                    .version_paint(project.reference_version, reference.model.text());
                self.ideas_changed();
            }
        }
        Task::none()
    }

    /// Applies `change` to the paint of the selected text, the draft's or else the left pane's. In
    /// the draft it is one undo step. In the left pane it changes that version's paint (its text
    /// can't change, but its paint can), and the draft gets the same change only where it still
    /// has the same words at the same place, as right after a Rewrite.
    fn change_selected_paint(&mut self, change: impl Fn(&mut Vec<Span>, Range<usize>)) {
        let Some(project) = &mut self.project else {
            return;
        };
        let draft = &mut self.pane;
        let reference = &mut project.reference;
        let in_draft = match (draft.selection(), reference.selection()) {
            (Some(range), _) => Some(range),
            (None, Some(range)) => {
                let text = reference.model.text().to_string();
                change(&mut reference.model.paint, range.clone());
                let number = project.reference_version;
                let paint = &reference.model.paint;
                project.project.set_version_paint(number, paint, &text);
                let same_place = draft.model.text().get(range.clone()) == text.get(range.clone());
                same_place.then_some(range)
            }
            (None, None) => return,
        };
        if let Some(range) = in_draft {
            let mut paint = draft.model.paint.clone();
            change(&mut paint, range);
            draft.model.set_paint(paint);
        }
        draft.deselect();
        reference.deselect();
        self.ideas_changed();
    }

    /// After any change to the ideas or the paint: weigh again, redraw the paint, and autosave
    /// (the manifest is saved with the draft).
    pub fn ideas_changed(&mut self) {
        let now = std::time::Instant::now();
        self.autosaver.edited(now);
        self.saving_since.get_or_insert(now);
        self.refresh_ideas();
    }
}
