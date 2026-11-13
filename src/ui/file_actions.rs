//! App's handlers for the file shortcuts: opening a document in this window (Ctrl+O, Ctrl+P,
//! Recent and Ctrl+0), New file (Ctrl+N), Save (Ctrl+S), Save as (Ctrl+Shift+S) and, in scratch,
//! Save selection (Ctrl+Shift+S there).

use std::path::PathBuf;
use std::time::Instant;

use iced::Task;
use iced::widget::operation::focus;

use crate::core::opened::{Doc, empty_file, open_in_place, opened_remote, save_as};
use crate::core::recent::same_file;
use crate::core::remote_file;
use crate::core::rewrite::remove_draft;
use crate::core::save::failure_reason;
use crate::core::scratch::clear_scratch;
use crate::core::settings::ScratchSyntax;
use crate::core::shutdown;
use crate::core::text_file::{LineEnding, save_text, suggested_file_name};
use crate::ui::app::{App, Message, Opening};
use crate::ui::editor;
use crate::ui::status_bar::keys;

impl App {
    /// Opens the file at `path` in this window (`open_here`). A file on a host comes as
    /// `alias:/path`, as the picker and the recent list name one.
    pub fn open_file(&mut self, path: PathBuf) -> Task<Message> {
        self.open_here(Doc::recent(&path))
    }

    /// Shows `doc` in this window in place of the document it shows now (Ctrl+O, Ctrl+P, Recent,
    /// Ctrl+N's new file and Ctrl+0), once that one is saved. If the save fails, or `doc` can't
    /// be opened, the window stays as it is and the status bar says why. The document already
    /// shown is left as it is. A file on a host is fetched first (`open_remote`).
    pub fn open_here(&mut self, doc: Doc) -> Task<Message> {
        self.close_panels();
        // Whatever was on its way to this window is called off: this is where it goes now.
        self.opening = None;
        self.call_off_fetch();
        let shown = match (&self.doc, &doc) {
            (Doc::Scratch, Doc::Scratch) => true,
            (Doc::File { path: current, .. }, Doc::File { path, .. }) => same_file(current, path),
            (
                Doc::Remote {
                    remote: current, ..
                },
                Doc::Remote { remote, .. },
            ) => current == remote,
            _ => false,
        };
        match doc {
            _ if shown => Task::none(),
            Doc::Remote { remote, .. } => self.open_remote(remote),
            doc => self.leave_for(Opening::Doc(doc)),
        }
    }

    /// Shows `next` in the window's place once the document it shows is saved (see
    /// `open_here`).
    pub fn leave_for(&mut self, next: Opening) -> Task<Message> {
        // A file on a host is saved off the UI thread, as an autosave is: its `Saved` comes
        // back here.
        // Unless the host didn't take it a moment ago (`leave_unsent`): then it stays on this
        // computer, written below as a file here is.
        let unsaved = self.editable() && !self.autosaver.close_allowed() && !self.disk_conflict;
        let send = matches!(self.doc, Doc::Remote { .. }) && unsaved && !self.leave_unsent;
        if send && !self.autosaver.is_saving() {
            self.opening = Some(next);
            return self.save(Instant::now());
        }
        if !self.saved_for_leaving() {
            return Task::none();
        }
        // An autosave still running is waited for: its `Saved` comes back here.
        if self.autosaver.is_saving() {
            self.opening = Some(next);
            return Task::none();
        }
        let opened = match next {
            Opening::Doc(doc) => open_in_place(&self.doc, doc, &self.paths),
            Opening::Fetched(fetched) => opened_remote(&self.doc, fetched, &self.paths),
        };
        match opened {
            Ok(opened) => self.show(opened),
            Err(reason) => {
                self.set_notice(Some(reason));
                Task::none()
            }
        }
    }

    /// Saves the document shown so the window can leave it, and says whether it may: not over a
    /// disk conflict, and not when the save fails (the status bar says why). An autosave still
    /// running is left to finish, since a save started now would land before it, under older
    /// text.
    fn saved_for_leaving(&mut self) -> bool {
        // Leaving would drop the text the save refused, without the status bar's choice.
        if self.disk_conflict {
            let words = "The file changed on disk: choose Reload or Keep mine first";
            self.set_notice(Some(words.to_string()));
            return false;
        }
        // A file on a host can't be sent while the window waits, as one here is written below
        // (`leave_for` sends it first; this is for Ctrl+N, whose dialog has given it time).
        if let Doc::Remote { remote, .. } = &self.doc
            && !self.leave_unsent
            && self.editable()
            && !self.autosaver.close_allowed()
            && !self.autosaver.is_saving()
        {
            let words = format!("Not saved to {} yet: try again once it is", remote.host);
            self.set_notice(Some(words));
            return false;
        }
        // Saved unless closing could skip it (so a failed last save is tried again). Scratch keeps
        // its cursor too, so it is written even then. A read-only window has nothing to save.
        let unsaved = !self.autosaver.close_allowed() || matches!(self.doc, Doc::Scratch);
        let saving = self.autosaver.is_saving();
        !self.editable() || saving || !unsaved || self.save_before_leaving()
    }

    /// Ctrl+N: asks where the new file goes; `NewFilePicked` brings the answer.
    pub fn new_file(&mut self) -> Task<Message> {
        self.menu_open = false;
        let dialog = self.save_dialog("New file", "untitled.txt".to_string());
        let document = self.document;
        Task::perform(dialog.save_file(), |picked| picked).and_then(move |file| {
            Task::done(Message::NewFilePicked(document, file.path().to_path_buf()))
        })
    }

    /// Starts the new file at `path` empty (the dialog has already agreed to replace anything
    /// there) and opens it (`open_file`). Nothing is emptied while the window can't leave the
    /// document it shows, since it would then stay on that one beside a file emptied for nothing.
    pub fn start_new_file(&mut self, path: PathBuf) -> Task<Message> {
        if !self.saved_for_leaving() {
            return Task::none();
        }
        match empty_file(&self.paths.data_dir, &self.doc, &path) {
            Ok(()) => self.open_file(path),
            Err(reason) => {
                self.set_notice(Some(reason));
                Task::none()
            }
        }
    }

    /// Ctrl+S. Everything autosaves, so in a file this writes now instead of after the pause.
    /// Scratch has no file, so it asks where to save one (Save as). While rewriting, it writes
    /// the draft over the original.
    pub fn save_now(&mut self) -> Task<Message> {
        if self.original.is_some() {
            return match self.editable() {
                true => self.save_rewrite(),
                false => Task::none(),
            };
        }
        if matches!(self.doc, Doc::Scratch) {
            return self.pick_save_path();
        }
        // Not over a file that changed on disk (the status bar asks what to do).
        let waiting = self.autosaver.pending() && !self.autosaver.is_saving();
        if self.editable() && waiting && !self.disk_conflict {
            return self.save(Instant::now());
        }
        Task::none()
    }

    /// Save as: asks where to save this text; `SaveAsPicked` brings the answer.
    pub fn pick_save_path(&mut self) -> Task<Message> {
        self.menu_open = false;
        let name = match &self.doc {
            Doc::Scratch => suggested_file_name(self.pane.model.text(), self.scratch_extension()),
            doc => doc
                .name()
                .and_then(|name| Some(name.file_name()?.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "untitled.txt".to_string()),
        };
        let dialog = self.save_dialog("Save as", name);
        let document = self.document;
        Task::perform(dialog.save_file(), |picked| picked).and_then(move |file| {
            Task::done(Message::SaveAsPicked(document, file.path().to_path_buf()))
        })
    }

    /// Makes `path` this window's file: the text is written there and the window carries on
    /// editing it, undo history and all. Scratch is emptied, keeping its text for the next scratch
    /// window's Ctrl+Z; a file keeps its last save. A rewrite ends, its draft going to `path`.
    pub fn save_as(&mut self, path: PathBuf) -> Task<Message> {
        // An autosave still running would land after the writes below: over scratch just
        // emptied, or over the old file with older text. So wait for it: its `Saved` comes back
        // here.
        if self.autosaver.is_saving() {
            self.opening = None;
            self.saving_as = Some(path);
            return Task::none();
        }
        let (line_ending, bom) = match &self.doc {
            Doc::File {
                path: current,
                line_ending,
                bom,
            } => {
                // Its own file: save, even with nothing new, so a file moved or deleted
                // outside the app is written again.
                if same_file(current, &path) {
                    return match self.editable() {
                        true => self.save(Instant::now()),
                        false => Task::none(),
                    };
                }
                (*line_ending, *bom)
            }
            Doc::Remote {
                line_ending, bom, ..
            } => (*line_ending, *bom),
            Doc::Scratch => (LineEnding::default(), false),
        };
        let data_dir = &self.paths.data_dir;
        let text = self.pane.model.text();
        // Typing not autosaved yet still goes to the file it was typed in (a rewrite's draft
        // goes to the new file instead).
        if let Doc::File { path: current, .. } = &self.doc
            && self.editable()
            && self.autosaver.pending()
            && self.original.is_none()
        {
            let _ = save_text(current, text, line_ending, bom, self.disk_stamp);
        }
        match save_as(data_dir, &self.doc, &path, text, line_ending, bom) {
            Err(reason) => self.set_notice(Some(reason)),
            Ok((lock, stamp)) => {
                self.set_notice(match self.doc {
                    Doc::Scratch if self.editable() => {
                        clear_scratch(data_dir, text, self.pane.model.history())
                            .err()
                            .map(|error| failure_reason(&error))
                    }
                    _ => None,
                });
                // A rewrite saved as a new file is done: the original keeps its last save.
                self.stale_draft = None;
                self.paused_draft = false;
                if self.original.take().is_some()
                    && let Some(current) = self.doc.path()
                {
                    let _ = remove_draft(&self.paths.data_dir, current);
                }
                // A file on a host keeps its last save there. The text has a home in the new
                // file now, so none of it waits on this computer to be sent.
                if let Doc::Remote { remote, .. } = &self.doc {
                    remote_file::forget(&self.paths.data_dir, remote, true);
                }
                self._doc_lock = lock;
                self.doc = Doc::File {
                    path,
                    line_ending,
                    bom,
                };
                self.disk_stamp = stamp;
                self.disk_conflict = false;
                self.read_only_reason = None;
                self.autosaver.mark_saved();
                // Typing autosave hadn't written is in the new file now. Left remembered, the
                // end of the app would write it back into scratch, just emptied.
                shutdown::forget();
                self.saving_since = None;
            }
        }
        Task::none()
    }

    /// Save selection (Ctrl+Shift+S in scratch): asks where the selected text goes;
    /// `SelectionPicked` brings the answer, with the text as it is selected now.
    pub fn pick_selection_path(&mut self) -> Task<Message> {
        self.menu_open = false;
        let Some(range) = self.pane.selection() else {
            let words = keys("Select the text to save first (Ctrl+S saves all of scratch)");
            self.set_notice(Some(words));
            return Task::none();
        };
        let text = self.pane.model.text()[range.clone()].to_string();
        let name = suggested_file_name(&text, self.scratch_extension());
        let dialog = self.save_dialog("Save selection as", name);
        let document = self.document;
        Task::perform(dialog.save_file(), |picked| picked).and_then(move |file| {
            let path = file.path().to_path_buf();
            let picked = Message::SelectionPicked(document, path, range.start, text.clone());
            Task::done(picked)
        })
    }

    /// Writes `text`, which was selected at byte `start` of scratch when the dialog opened, to
    /// `path` as a file of its own, takes it out of scratch as one undo step (so Ctrl+Z in
    /// scratch puts it back), then opens the file in this window (`open_file`), which saves
    /// scratch first. The text leaves scratch only once the file holds it, and nothing is written
    /// if scratch no longer has that text there.
    pub fn save_selection(&mut self, path: PathBuf, start: usize, text: String) -> Task<Message> {
        let range = start..start + text.len();
        let unchanged = self.pane.model.text().get(range.clone()) == Some(text.as_str());
        if !matches!(self.doc, Doc::Scratch) || !self.editable() || !unchanged {
            let words = "That text changed while the dialog was open, so nothing was saved";
            self.set_notice(Some(words.to_string()));
            return Task::none();
        }
        let data_dir = &self.paths.data_dir;
        let ending = LineEnding::default();
        // The claim on the new file is let go at once, so that opening it can claim it again.
        match save_as(data_dir, &self.doc, &path, &text, ending, false).map(drop) {
            Err(reason) => {
                self.set_notice(Some(reason));
                focus(editor::EDITOR_ID)
            }
            Ok(()) => {
                self.pane.replace(range, "");
                self.text_changed();
                self.open_file(path)
            }
        }
    }

    /// The extension offered for a file saved from scratch: `md` while scratch is coloured as
    /// Markdown, so the file keeps its colours.
    fn scratch_extension(&self) -> &'static str {
        match self.settings.scratch_syntax {
            ScratchSyntax::Markdown => "md",
            ScratchSyntax::Plain => "txt",
        }
    }

    /// A save dialog titled `title`, offering `name`, starting beside the open file or in the
    /// documents folder. It has no file type filter, so a name is saved as typed: with one,
    /// macOS and Windows add the filter's first extension to a name it doesn't list, so test.sh
    /// became test.sh.txt.
    fn save_dialog(&self, title: &str, name: String) -> rfd::AsyncFileDialog {
        let folder = match &self.doc {
            Doc::File { path, .. } => path.parent(),
            Doc::Scratch | Doc::Remote { .. } => Some(self.paths.documents_dir.as_path()),
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(title)
            .set_file_name(name);
        match folder.filter(|folder| folder.is_dir()) {
            Some(folder) => dialog.set_directory(folder),
            None => dialog,
        }
    }
}
