//! App's handlers for the file shortcuts: opening a file (Ctrl+O, Ctrl+P and Recent), New file
//! (Ctrl+N), Save (Ctrl+S) and Save as (Ctrl+Shift+S).

use std::path::PathBuf;
use std::time::Instant;

use iced::Task;

use crate::core::opened::{Doc, empty_file, open_in_place, save_as};
use crate::core::recent::same_file;
use crate::core::save::failure_reason;
use crate::core::scratch::{ScratchState, clear_scratch};
use crate::core::text_file::{LineEnding, TEXT_EXTENSIONS, save_text, suggested_file_name};
use crate::ui::app::{App, Message, open_in_new_window};

impl App {
    /// Opens the file at `path`: from a file, in a window of its own; from scratch, in this window
    /// in scratch's place, once scratch's latest text and cursor are on disk. If that save fails,
    /// or the file can't be opened, the window stays on scratch and the status bar says why.
    pub fn open_file(&mut self, path: PathBuf) -> Task<Message> {
        self.menu_open = false;
        self.switcher = None;
        if !matches!(self.doc, Doc::Scratch) {
            self.set_notice(open_in_new_window(Some(path)).err());
            return Task::none();
        }
        // An autosave still running would land after the save below, over newer text, so wait
        // for it: its `Saved` comes back here.
        if self.autosaver.is_saving() {
            self.opening = Some(path);
            return Task::none();
        }
        // Written now, as the autosave would (a failure shows as one, which keeps the window as
        // it keeps a closing one). A read-only scratch belongs to another window: nothing to save.
        if self.editable() {
            let generation = self.autosaver.begin(Instant::now());
            let scratch = ScratchState {
                text: self.pane.model.text().to_string(),
                view: self.scratch_view(),
            };
            let saved = scratch.save(&self.paths.data_dir);
            let failed = saved.is_err();
            self.autosaver
                .finish(generation, saved.map_err(|error| failure_reason(&error)));
            if failed {
                return Task::none();
            }
        }
        match open_in_place(&path, &self.paths) {
            Ok(opened) => self.show(opened),
            Err(reason) => {
                self.set_notice(Some(reason));
                Task::none()
            }
        }
    }

    /// Ctrl+N: asks where the new file (.txt or .md) goes; `NewFilePicked` brings the answer.
    pub fn new_file(&mut self) -> Task<Message> {
        self.menu_open = false;
        let dialog = self.save_dialog("New file", "untitled.txt".to_string());
        Task::perform(dialog.save_file(), |picked| picked)
            .and_then(|file| Task::done(Message::NewFilePicked(file.path().to_path_buf())))
    }

    /// Starts the new file at `path` empty (the dialog has already agreed to replace anything
    /// there) and opens it (`open_file`).
    pub fn start_new_file(&mut self, path: PathBuf) -> Task<Message> {
        match empty_file(&self.paths.data_dir, &self.doc, &path) {
            Ok(()) => self.open_file(path),
            Err(reason) => {
                self.set_notice(Some(reason));
                Task::none()
            }
        }
    }

    /// Ctrl+S. Everything autosaves, so in a file this writes now instead of after the pause.
    /// Scratch has no file, so it asks where to save one (Save as).
    pub fn save_now(&mut self) -> Task<Message> {
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

    /// Save as: asks where to save this text as a .txt or .md; `SaveAsPicked` brings the answer.
    pub fn pick_save_path(&mut self) -> Task<Message> {
        self.menu_open = false;
        let name = match &self.doc {
            Doc::Scratch => suggested_file_name(self.pane.model.text()),
            Doc::File { path, .. } => path.file_name().map_or_else(
                || "untitled.txt".to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
        };
        let dialog = self.save_dialog("Save as", name);
        Task::perform(dialog.save_file(), |picked| picked)
            .and_then(|file| Task::done(Message::SaveAsPicked(file.path().to_path_buf())))
    }

    /// Makes `path` this window's file: the text is written there and the window carries on
    /// editing it, undo history and all. Scratch is emptied, keeping its text for the next scratch
    /// window's Ctrl+Z; a file keeps its last save.
    pub fn save_as(&mut self, path: PathBuf) -> Task<Message> {
        let (line_ending, bom) = match &self.doc {
            Doc::File {
                path: current,
                line_ending,
                bom,
            } => {
                // Its own file: save, even with nothing new, so a file moved or deleted
                // outside the app is written again.
                if same_file(current, &path) {
                    let can_save = self.editable() && !self.autosaver.is_saving();
                    return match can_save {
                        true => self.save(Instant::now()),
                        false => Task::none(),
                    };
                }
                (*line_ending, *bom)
            }
            _ => (LineEnding::default(), false),
        };
        let data_dir = &self.paths.data_dir;
        let text = self.pane.model.text();
        // Typing not autosaved yet still goes to the file it was typed in.
        if let Doc::File { path: current, .. } = &self.doc
            && self.editable()
            && self.autosaver.pending()
        {
            let _ = save_text(current, text, line_ending, bom, self.disk_stamp);
        }
        match save_as(data_dir, &self.doc, &path, text, line_ending, bom) {
            Err(reason) => self.set_notice(Some(reason)),
            Ok((lock, stamp)) => {
                self.set_notice(match self.doc {
                    Doc::Scratch if self.editable() => clear_scratch(data_dir, text)
                        .err()
                        .map(|error| failure_reason(&error)),
                    _ => None,
                });
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
                self.saving_since = None;
            }
        }
        Task::none()
    }

    /// A save dialog for a .txt or .md, titled `title`, offering `name`, starting beside the open
    /// file or in the documents folder. The filter lists every extension the app opens: macOS
    /// adds the first one (.txt) to a name that has none of them, so notes.md would become
    /// notes.md.txt without .md there, and Windows can do the same to one the filter leaves out.
    fn save_dialog(&self, title: &str, name: String) -> rfd::AsyncFileDialog {
        let folder = match &self.doc {
            Doc::File { path, .. } => path.parent(),
            Doc::Scratch => Some(self.paths.documents_dir.as_path()),
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(title)
            .add_filter("Text and Markdown", &TEXT_EXTENSIONS)
            .set_file_name(name);
        match folder.filter(|folder| folder.is_dir()) {
            Some(folder) => dialog.set_directory(folder),
            None => dialog,
        }
    }
}
