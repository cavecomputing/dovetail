//! App's handlers for a rewrite (Ctrl+R or the pen button): the document's text is copied into a
//! draft, edited on the right while the original stays read-only on the left. The draft autosaves
//! to its own file (`core/rewrite.rs`); only Ctrl+S writes it over the original.

use iced::Task;
use iced::widget::operation::focus;

use crate::core::counts::Counts;
use crate::core::opened::{Doc, write_doc};
use crate::core::rewrite::{read_draft, remove_draft, write_draft};
use crate::core::shutdown;
use crate::core::text_file::{DiskStamp, changed_on_disk, read_text};
use crate::ui::app::{App, Message};
use crate::ui::editor::{self, Pane};

impl App {
    /// Ctrl+R and the pen button: starts a rewrite of the document, or ends the one under way.
    /// An autosave running now is waited for, as its `Saved` comes back here: the original's
    /// would land after the rewrite started, and a draft's after its rewrite ended, bringing the
    /// draft back.
    pub fn toggle_rewrite(&mut self) -> Task<Message> {
        self.menu_open = false;
        if !self.editable() {
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
        match self.original.is_some() {
            true => self.end_rewrite(),
            false => self.start_rewrite(),
        }
        focus(editor::EDITOR_ID)
    }

    fn start_rewrite(&mut self) {
        // Typing not autosaved yet belongs to the original, where it was typed.
        if !self.autosaver.close_allowed() && !self.save_before_leaving() {
            return;
        }
        let text = self.pane.model.text().to_string();
        if let Err(error) = write_draft(&self.paths.data_dir, &self.doc, &text) {
            self.set_notice(Some(format!("Couldn't start a rewrite: {error}")));
            return;
        }
        // The draft starts its own undo history, so Ctrl+Z stays inside the rewrite.
        let mut draft = Pane::new(&text);
        draft.content.move_to(self.pane.content.cursor());
        draft
            .content
            .scroll_to_line(self.pane.content.scroll_line());
        self.pane = draft;
        self.show_original(&text);
    }

    /// Goes back to the original as last saved, dropping the draft's unsaved changes. The
    /// original's pane comes back with an undo history of its own, so no Ctrl+Z brings the draft
    /// into the document.
    fn end_rewrite(&mut self) {
        if let Err(error) = remove_draft(&self.paths.data_dir, &self.doc) {
            self.set_notice(Some(format!("Couldn't end the rewrite: {error}")));
            return;
        }
        let Some(original) = self.original.take() else {
            return;
        };
        if self.pane.model.text() != original.model.text() {
            self.pane = original;
            self.text_changed();
        }
        // The document already holds that text.
        self.autosaver.mark_saved();
        self.saving_since = None;
        shutdown::forget();
    }

    /// Ctrl+S while rewriting: writes the draft over the original, which the left side then
    /// shows. A file changed by another program since is left alone, and the left side shows it
    /// as it is now, so a second Ctrl+S writes over what can be seen.
    pub fn save_rewrite(&mut self) {
        let text = self.pane.model.text().to_string();
        let data_dir = &self.paths.data_dir;
        let view = self.scratch_view();
        match write_doc(&self.doc, text.clone(), view, data_dir, self.disk_stamp) {
            Ok(stamp) => {
                if stamp.is_some() {
                    self.disk_stamp = stamp;
                }
                self.show_original(&text);
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
                    self.show_original(&file.text);
                    let words = "The file changed on disk: the left shows it now, Ctrl+S again \
                                 writes over it";
                    self.set_notice(Some(words.to_string()));
                } else {
                    self.set_notice(Some(reason));
                }
            }
        }
    }

    /// Brings back a rewrite of the document left open (its draft is still there), once the
    /// document is shown. A window that can't edit the document leaves the draft for one that can.
    pub fn resume_rewrite(&mut self) {
        self.original = None;
        if !self.editable() {
            return;
        }
        if let Some(draft) = read_draft(&self.paths.data_dir, &self.doc) {
            let original = std::mem::replace(&mut self.pane, Pane::new(&draft));
            self.original = Some(original);
            self.counts = Counts::of(&draft);
        }
    }

    /// Shows `text` as the original on the left, scrolled to where the draft is.
    fn show_original(&mut self, text: &str) {
        let mut original = Pane::new(text);
        original
            .content
            .scroll_to_line(self.pane.content.scroll_line());
        self.original = Some(original);
    }
}
