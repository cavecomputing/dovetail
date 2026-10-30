//! App's handlers for a rewrite (Ctrl+R or the pen button): the document's text is copied into a
//! draft, edited on the right while the original stays read-only on the left. The draft autosaves
//! to its own file (`core/rewrite.rs`); only Ctrl+S writes it over the original.

use iced::Task;
use iced::widget::operation::focus;

use crate::core::counts::Counts;
use crate::core::opened::{Doc, write_doc};
use crate::core::rewrite::{
    Changes, base_changed, changes, has_paused, pause_draft, read_draft, remove_draft, take_paused,
    write_base, write_draft,
};
use crate::core::shutdown;
use crate::core::text_file::{DiskStamp, changed_on_disk, read_text};
use crate::core::text_model::TextModel;
use crate::ui::app::{App, Message};
use crate::ui::editor::{self, Pane};

/// Why Ctrl+S waits in a rewrite whose file another program changed while the draft was closed
/// or paused, and the same once the first Ctrl+S was refused.
const STALE_DRAFT: &str = "The file changed since this rewrite started: the left shows it now";
const STALE_DRAFT_AGAIN: &str =
    "The file changed since this rewrite started: Ctrl+S again writes over it";

impl App {
    /// Whether the document can be rewritten: a file this window can edit (scratch has no
    /// rewrite).
    pub fn can_rewrite(&self) -> bool {
        self.editable() && self.doc.path().is_some()
    }

    /// Ctrl+R and the pen button: starts a rewrite of the file (picking up a paused one), or
    /// pauses the one under way.
    /// An autosave running now is waited for, as its `Saved` comes back here: the original's
    /// would land after the rewrite started, and a draft's after its rewrite ended, bringing the
    /// draft back.
    pub fn toggle_rewrite(&mut self) -> Task<Message> {
        self.menu_open = false;
        if !self.can_rewrite() {
            // Scratch has no file to rewrite; a file another window holds can be, there.
            let words = match self.doc.path() {
                Some(_) => "Rewrite needs a file this window can edit",
                None => "Rewrite needs a saved file: Ctrl+S saves scratch to one",
            };
            self.set_notice(Some(words.to_string()));
            return Task::none();
        }
        if self.disk_conflict {
            let words = "The file changed on disk: choose Reload or Keep mine first";
            self.set_notice(Some(words.to_string()));
            return Task::none();
        }
        if self.autosaver.is_saving() {
            self.toggling_rewrite = true;
            return Task::none();
        }
        let toggled = match self.original.is_some() {
            true => {
                self.pause_rewrite();
                Task::none()
            }
            false => self.start_rewrite(),
        };
        Task::batch([toggled, focus(editor::EDITOR_ID)])
    }

    fn start_rewrite(&mut self) -> Task<Message> {
        // Typing not autosaved yet belongs to the original, where it was typed.
        if !self.autosaver.close_allowed() && !self.save_before_leaving() {
            return Task::none();
        }
        let Some(path) = self.doc.path() else {
            return Task::none();
        };
        let saved = self.pane.model.text().to_string();
        let data_dir = &self.paths.data_dir;
        let paused = take_paused(data_dir, path);
        self.paused_draft = false;
        self.stale_draft = None;
        if paused.is_some() && base_changed(data_dir, path, &saved) {
            self.stale_draft = Some(STALE_DRAFT);
        }
        let text = match paused {
            Some(paused) => paused,
            None => match write_draft(data_dir, path, &saved)
                .and_then(|()| write_base(data_dir, path, &saved))
            {
                Ok(()) => saved.clone(),
                Err(error) => {
                    self.set_notice(Some(format!("Couldn't start a rewrite: {error}")));
                    return Task::none();
                }
            },
        };
        // The draft starts its own undo history, so Ctrl+Z stays inside the rewrite. A new one
        // keeps the caret and the view where they were. The caret is taken as an offset of the
        // text, not the widget's position: with suggestions waiting, the widget shows their
        // added text too, and a position past it would be off the end of the draft's line.
        let mut draft = Pane::new(&text);
        if text == saved {
            draft.put_caret(self.pane.caret());
            draft
                .content
                .scroll_to_line(self.pane.content.scroll_line());
        }
        self.pane = draft;
        // A paused draft isn't the text counted, checked and searched so far.
        let checked = match text == saved {
            true => Task::none(),
            false => Task::batch([self.count(), self.check_syntax()]),
        };
        if self.find.open {
            self.find_again(self.pane.caret());
        }
        Task::batch([self.show_original(&saved), checked])
    }

    /// Goes back to the original as last saved, keeping the draft aside for the next Ctrl+R (a
    /// draft no different from the original is just deleted). The original's pane comes back
    /// with an undo history of its own, so no Ctrl+Z brings the draft into the document. The
    /// caret and the view stay where they were in the draft.
    fn pause_rewrite(&mut self) {
        let Some(original) = self.original.take() else {
            return;
        };
        let draft = self.pane.model.text();
        let changed = draft != original.model.text();
        let caret = self.pane.caret();
        let top = self.pane.content.scroll_line();
        if let Some(path) = self.doc.path() {
            let data_dir = &self.paths.data_dir;
            let kept = match changed {
                true => pause_draft(data_dir, path, draft),
                false => remove_draft(data_dir, path),
            };
            if let Err(error) = kept {
                self.original = Some(original);
                self.set_notice(Some(format!("Couldn't pause the rewrite: {error}")));
                return;
            }
        }
        self.pane = original;
        self.stale_draft = None;
        self.paused_draft = changed;
        let caret = self.pane.model.text().floor_char_boundary(caret);
        self.pane.put_caret(caret);
        self.pane.content.scroll_to_line(top);
        // A comparison still running is of the draft; its answer is dropped.
        self.changes = Changes::default();
        self.diff_generation += 1;
        if changed {
            self.text_changed();
        }
        // The document already holds that text.
        self.autosaver.mark_saved();
        self.saving_since = None;
        shutdown::forget();
    }

    /// Ctrl+Shift+R and the menu's "reset rewrite": the draft goes back to the file as last
    /// saved, the split staying. It is one undo step, so Ctrl+Z takes a reset back.
    pub fn reset_rewrite(&mut self) {
        self.menu_open = false;
        let Some(original) = &self.original else {
            return;
        };
        let saved = original.model.text().to_string();
        if self.pane.model.text() != saved {
            // Replacing the whole text leaves the caret at its end; keep it and the view where
            // they were instead.
            let caret = saved.floor_char_boundary(self.pane.caret());
            let top = self.pane.content.scroll_line();
            let length = self.pane.model.text().len();
            self.pane.replace(0..length, &saved);
            self.pane.put_caret(caret);
            self.pane.content.scroll_to_line(top);
            self.text_changed();
        }
    }

    /// Ctrl+S while rewriting: writes the draft over the original, which the left side then
    /// shows. A file changed by another program since is left alone, and the left side shows it
    /// as it is now, so a second Ctrl+S writes over what can be seen.
    /// A draft picked up again after its file changed elsewhere waits for a second Ctrl+S too.
    pub fn save_rewrite(&mut self) -> Task<Message> {
        if self.stale_draft == Some(STALE_DRAFT) {
            self.stale_draft = Some(STALE_DRAFT_AGAIN);
            return Task::none();
        }
        let text = self.pane.model.text().to_string();
        let data_dir = &self.paths.data_dir;
        let view = self.scratch_view();
        match write_doc(&self.doc, text.clone(), view, data_dir, self.disk_stamp) {
            Ok(stamp) => {
                if stamp.is_some() {
                    self.disk_stamp = stamp;
                }
                if let Some(path) = self.doc.path() {
                    let _ = write_base(data_dir, path, &text);
                }
                self.stale_draft = None;
                self.show_original(&text)
            }
            Err(reason) => {
                if let Doc::File {
                    path,
                    line_ending,
                    bom,
                } = &mut self.doc
                    && changed_on_disk(path, self.disk_stamp)
                    && let Ok(file) = read_text(path)
                {
                    *line_ending = file.line_ending;
                    *bom = file.bom;
                    self.disk_stamp = DiskStamp::of(path);
                    let words = "The file changed on disk: the left shows it now, Ctrl+S again \
                                 writes over it";
                    self.set_notice(Some(words.to_string()));
                    self.show_original(&file.text)
                } else {
                    self.set_notice(Some(reason));
                    Task::none()
                }
            }
        }
    }

    /// Brings back a rewrite of the document left open (its draft is still there), once the
    /// document is shown. A window that can't edit the document leaves the draft for one that can.
    /// A file changed elsewhere since the draft started from it is said so (`STALE_DRAFT`).
    pub fn resume_rewrite(&mut self) -> Task<Message> {
        self.original = None;
        self.stale_draft = None;
        self.paused_draft =
            (self.doc.path()).is_some_and(|path| has_paused(&self.paths.data_dir, path));
        if self.editable()
            && let Some(path) = self.doc.path()
            && let Some(draft) = read_draft(&self.paths.data_dir, path)
        {
            if base_changed(&self.paths.data_dir, path, self.pane.model.text()) {
                self.stale_draft = Some(STALE_DRAFT);
            }
            let original = std::mem::replace(&mut self.pane, Pane::new(&draft));
            self.original = Some(original);
            self.counts = Counts::of(&draft);
        }
        Task::batch([self.find_changes(), self.count_original()])
    }

    /// Works out what the draft changes from the original, for lighting both sides. The
    /// comparison runs off the UI thread (it can take a while on a long text with many changes)
    /// and `Changed` brings the answer back; an answer to an older text is dropped. Until it
    /// comes, the ranges lit now move along with the typing (`shift_changes`).
    pub fn find_changes(&mut self) -> Task<Message> {
        self.diff_generation += 1;
        let Some(original) = &self.original else {
            self.changes = Changes::default();
            return Task::none();
        };
        let draft = self.pane.model.text().to_string();
        self.changes_text = draft.clone();
        if original.model.text() == draft {
            self.changes = Changes::default();
            return Task::none();
        }
        let original = original.model.text().to_string();
        let generation = self.diff_generation;
        Task::perform(async move { changes(&original, &draft) }, move |found| {
            Message::Changed(generation, found)
        })
    }

    /// Moves the lit ranges along with an edit to the draft, so they stay on their letters until
    /// the next comparison (which `Tick` starts after the typing pauses, with the counts).
    pub fn shift_changes(&mut self) {
        if self.original.is_none() {
            return;
        }
        // A comparison under way is of the text before this edit.
        self.diff_generation += 1;
        let text = self.pane.model.text();
        let edit = TextModel::change_between(&self.changes_text, text, self.pane.caret());
        if let Some(edit) = edit {
            self.changes
                .shift(edit.at, edit.removed.len(), edit.inserted.len());
        }
        self.changes_text = text.to_string();
    }

    /// Whether the draft holds anything Ctrl+S hasn't written to the file yet.
    pub fn rewrite_unsaved(&self) -> bool {
        (self.original.as_ref())
            .is_some_and(|original| original.model.text() != self.pane.model.text())
    }

    /// Shows `text` as the original on the left, scrolled to where the draft is.
    fn show_original(&mut self, text: &str) -> Task<Message> {
        let mut original = Pane::new(text);
        original
            .content
            .scroll_to_line(self.pane.content.scroll_line());
        self.original = Some(original);
        Task::batch([self.find_changes(), self.count_original()])
    }

    /// Counts the original off the UI thread, for the status bar's "original → draft"; it only
    /// changes when the rewrite starts or Ctrl+S writes it.
    fn count_original(&mut self) -> Task<Message> {
        self.original_counts = None;
        let Some(original) = &self.original else {
            return Task::none();
        };
        let text = original.model.text().to_string();
        let document = self.document;
        Task::perform(async move { Counts::of(&text) }, move |counts| {
            Message::OriginalCounted(document, counts)
        })
    }
}
