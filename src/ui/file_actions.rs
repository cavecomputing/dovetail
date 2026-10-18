//! App's handlers for the file shortcuts: New file (Ctrl+N), Save (Ctrl+S) and Save as
//! (Ctrl+Shift+S).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use iced::Task;

use crate::core::opened::{Doc, empty_file, save_as};
use crate::core::save::failure_reason;
use crate::core::scratch::clear_scratch;
use crate::core::text_file::{LineEnding, save_text, suggested_file_name};
use crate::ui::app::{App, Message, open_in_new_window};

impl App {
    /// Ctrl+N: asks where the new .txt goes; `NewFilePicked` brings the answer.
    pub fn new_file(&mut self) -> Task<Message> {
        self.menu_open = false;
        let dialog = self.save_dialog("New file", "untitled.txt".to_string());
        Task::perform(dialog.save_file(), |picked| picked)
            .and_then(|file| Task::done(Message::NewFilePicked(file.path().to_path_buf())))
    }

    /// Starts the new file at `path` empty (the dialog has already agreed to replace anything
    /// there) and opens it in its own window.
    pub fn start_new_file(&mut self, path: PathBuf) -> Task<Message> {
        self.notice = match empty_file(&self.paths.data_dir, &path) {
            Ok(()) => open_in_new_window(Some(path)).err(),
            Err(reason) => Some(reason),
        };
        Task::none()
    }

    /// Ctrl+S. Everything autosaves, so in a .txt or a project this writes now instead of after
    /// the pause. Scratch has no file, so it asks where to save one (Save as).
    pub fn save_now(&mut self) -> Task<Message> {
        if matches!(self.doc, Doc::Scratch) {
            return self.pick_save_path();
        }
        // Not while git is busy (a commit writes the draft itself), and not over a file that
        // changed on disk (the status bar asks what to do).
        let waiting = self.autosaver.pending() && !self.autosaver.is_saving();
        if self.editable() && waiting && !self.busy && !self.disk_conflict {
            return self.save(Instant::now());
        }
        Task::none()
    }

    /// Save as: asks where to save this text as a .txt; `SaveAsPicked` brings the answer. A
    /// project saves into its own folder, so it has no Save as.
    pub fn pick_save_path(&mut self) -> Task<Message> {
        self.menu_open = false;
        let name = match &self.doc {
            Doc::Scratch => suggested_file_name(self.pane.model.text()),
            Doc::File { path, .. } => path.file_name().map_or_else(
                || "untitled.txt".to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            Doc::Project(_) => return Task::none(),
        };
        let dialog = self.save_dialog("Save as", name);
        Task::perform(dialog.save_file(), |picked| picked)
            .and_then(|file| Task::done(Message::SaveAsPicked(file.path().to_path_buf())))
    }

    /// Makes `path` this window's file: the text is written there and the window carries on
    /// editing it, undo history and all. Scratch is emptied (as a Rewrite does), keeping its text
    /// for the next scratch window's Ctrl+Z; a .txt keeps its last save.
    pub fn save_as(&mut self, path: PathBuf) -> Task<Message> {
        let line_ending = match &self.doc {
            Doc::File {
                path: current,
                line_ending,
            } => {
                // Its own file: save, even with nothing new, so a file moved or deleted
                // outside the app is written again.
                if same_file(current, &path) {
                    let can_save = self.editable() && !self.busy && !self.autosaver.is_saving();
                    return match can_save {
                        true => self.save(Instant::now()),
                        false => Task::none(),
                    };
                }
                *line_ending
            }
            _ => LineEnding::default(),
        };
        let data_dir = &self.paths.data_dir;
        let text = self.pane.model.text();
        // Typing not autosaved yet still goes to the file it was typed in.
        if let Doc::File {
            path: current,
            line_ending,
        } = &self.doc
            && self.editable()
            && self.autosaver.pending()
        {
            let _ = save_text(current, text, *line_ending, self.disk_stamp);
        }
        match save_as(data_dir, &path, text, line_ending) {
            Err(reason) => self.notice = Some(reason),
            Ok((lock, stamp)) => {
                self.notice = match self.doc {
                    Doc::Scratch if self.editable() => clear_scratch(data_dir, text)
                        .err()
                        .map(|error| failure_reason(&error)),
                    _ => None,
                };
                self._doc_lock = lock;
                self.doc = Doc::File { path, line_ending };
                self.disk_stamp = stamp;
                self.disk_conflict = false;
                self.read_only_reason = None;
                self.autosaver.mark_saved();
                self.saving_since = None;
                self.saved_at = Some(Instant::now());
            }
        }
        Task::none()
    }

    /// A save dialog for a .txt, titled `title`, offering `name`, starting beside the open file
    /// or in the documents folder.
    fn save_dialog(&self, title: &str, name: String) -> rfd::AsyncFileDialog {
        let folder = match &self.doc {
            Doc::File { path, .. } => path.parent(),
            _ => self.paths.projects_dir.parent(),
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(title)
            .add_filter("Text", &["txt"])
            .set_file_name(name);
        match folder.filter(|folder| folder.is_dir()) {
            Some(folder) => dialog.set_directory(folder),
            None => dialog,
        }
    }
}

/// Whether `a` and `b` are the same file on disk (through any links), or the same path when one
/// is missing.
fn same_file(a: &Path, b: &Path) -> bool {
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real(a) == real(b)
}
