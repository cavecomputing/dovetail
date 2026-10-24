//! The scratch space: one continuous document, resumed on launch exactly where it was left.
//!
//! The text lives in `scratch.txt` (plain, readable without the app) and the cursor in a small
//! `scratch.json` next to it, both in the data folder. The view comes back with the same line at
//! the top.
//!
//! Saving scratch to a file empties it. The text it held is kept in `scratch-cleared.txt` until
//! the next scratch window opens, which takes it back as its first undo step: scratch opens empty,
//! and Ctrl+Z brings the text back.

use std::fs::{self, TryLockError};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::lock::{Lock, claim_lock, locked_elsewhere};
use crate::core::save::atomic_write;
use crate::core::text_file::read_text;

const TEXT_FILE: &str = "scratch.txt";
const VIEW_FILE: &str = "scratch.json";
const LOCK_FILE: &str = "scratch.lock";
const CLEARED_FILE: &str = "scratch-cleared.txt";

/// Where the cursor was. Both count from zero; the column is a byte offset into the line (what
/// iced reports), not a count of characters. They may be past the end if the text was edited
/// outside the app, so `ScratchState::cursor` clamps them.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScratchView {
    pub cursor_line: usize,
    pub cursor_column: usize,
    /// The line at the top of the view; `None` in files saved before it was recorded.
    pub scroll_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScratchState {
    pub text: String,
    pub view: ScratchView,
}

impl ScratchState {
    /// Loads the scratch space from `data_dir`. No saved scratch yet gives an empty one, and a
    /// broken `scratch.json` just loses the cursor position. Failing to read the text itself is an
    /// error, so the caller doesn't autosave an empty scratch over it.
    pub fn load(data_dir: &Path) -> io::Result<ScratchState> {
        let text = match read_text(&data_dir.join(TEXT_FILE)) {
            Ok(file) => file.text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let view = fs::read_to_string(data_dir.join(VIEW_FILE))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Ok(ScratchState { text, view })
    }

    /// Saves the text and the view, each atomically.
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        atomic_write(&data_dir.join(TEXT_FILE), self.text.as_bytes())?;
        self.view.save(data_dir)
    }

    /// The saved cursor as (line, byte column), kept inside the text and on a character boundary.
    pub fn cursor(&self) -> (usize, usize) {
        let lines: Vec<&str> = self.text.split('\n').collect();
        let line = self.view.cursor_line.min(lines.len() - 1);
        (line, clamp_column(lines[line], self.view.cursor_column))
    }
}

/// `column` (a byte offset) limited to `line`'s length and moved back to the start of the
/// character it falls in. A cursor inside a multi-byte character such as `…` would crash the
/// editor on the next keystroke.
pub fn clamp_column(line: &str, column: usize) -> usize {
    let mut column = column.min(line.len());
    while !line.is_char_boundary(column) {
        column -= 1;
    }
    column
}

impl ScratchView {
    /// Saves only the cursor, for when they move but the text hasn't changed.
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let json = serde_json::to_string(self).map_err(io::Error::other)?;
        atomic_write(&data_dir.join(VIEW_FILE), json.as_bytes())
    }
}

/// Empties scratch after its text was saved to a file, keeping `text` (what it held) for the next
/// scratch window to undo back to.
pub fn clear_scratch(data_dir: &Path, text: &str) -> io::Result<()> {
    atomic_write(&data_dir.join(CLEARED_FILE), text.as_bytes())?;
    ScratchState::default().save(data_dir)
}

/// The text scratch was emptied of, if any, removed so it is only offered once.
pub fn take_cleared(data_dir: &Path) -> Option<String> {
    let path = data_dir.join(CLEARED_FILE);
    let text = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    Some(text)
}

/// True when another running process has the scratch space open.
pub fn scratch_open_elsewhere(data_dir: &Path) -> bool {
    locked_elsewhere(&scratch_lock_path(data_dir))
}

/// The lock that keeps scratch to one window; anything else that would write scratch's text
/// file claims it too.
pub fn scratch_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCK_FILE)
}

/// Where scratch's text is kept.
pub fn scratch_text_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TEXT_FILE)
}

/// Claims the scratch space for this process, so a second window can't write over it. Keep the
/// returned `File` open while the window lives; the claim ends with the process.
/// `Err(TryLockError::WouldBlock)` means another window already has it.
pub fn claim_scratch(data_dir: &Path) -> Result<Lock, TryLockError> {
    claim_lock(&scratch_lock_path(data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_saved_gives_empty_scratch() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ScratchState::load(dir.path()).unwrap(),
            ScratchState::default()
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let state = ScratchState {
            text: "hello\nworld".to_string(),
            view: ScratchView {
                cursor_line: 1,
                cursor_column: 3,
                scroll_line: Some(1),
            },
        };
        state.save(&data_dir).unwrap();
        assert_eq!(
            fs::read_to_string(data_dir.join(TEXT_FILE)).unwrap(),
            "hello\nworld"
        );
        assert_eq!(ScratchState::load(&data_dir).unwrap(), state);
    }

    #[test]
    fn broken_sidecar_keeps_the_text() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(TEXT_FILE), "keep me").unwrap();
        fs::write(dir.path().join(VIEW_FILE), "{not json").unwrap();
        let state = ScratchState::load(dir.path()).unwrap();
        assert_eq!(state.text, "keep me");
        assert_eq!(state.view, ScratchView::default());
    }

    #[test]
    fn unreadable_text_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(TEXT_FILE), [0xff, 0xff]).unwrap();
        assert!(ScratchState::load(dir.path()).is_err());
    }

    #[test]
    fn scratch_lock_is_held_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let claim = claim_scratch(dir.path()).unwrap();
        assert!(scratch_open_elsewhere(dir.path()));
        assert!(matches!(
            claim_scratch(dir.path()),
            Err(TryLockError::WouldBlock)
        ));
        drop(claim);
        assert!(!scratch_open_elsewhere(dir.path()));
    }

    #[test]
    fn a_lock_file_left_behind_is_not_a_claim() {
        let dir = tempfile::tempdir().unwrap();
        // What a crashed window leaves: the file, but no live lock on it.
        fs::write(dir.path().join(LOCK_FILE), "1").unwrap();
        assert!(!scratch_open_elsewhere(dir.path()));
        assert!(claim_scratch(dir.path()).is_ok());
    }

    #[test]
    fn clearing_keeps_the_old_text_once() {
        let dir = tempfile::tempdir().unwrap();
        ScratchState {
            text: "draft me".to_string(),
            view: ScratchView::default(),
        }
        .save(dir.path())
        .unwrap();
        clear_scratch(dir.path(), "draft me").unwrap();
        assert_eq!(ScratchState::load(dir.path()).unwrap().text, "");
        assert_eq!(take_cleared(dir.path()).as_deref(), Some("draft me"));
        assert_eq!(take_cleared(dir.path()), None);
    }

    #[test]
    fn column_is_clamped_in_bytes_to_a_character_boundary() {
        // "short…" is 6 characters but 8 bytes; byte 7 is inside the 3-byte "…".
        assert_eq!(clamp_column("short…", 7), 5);
        assert_eq!(clamp_column("short…", 8), 8);
        assert_eq!(clamp_column("short…", 99), 8);
        assert_eq!(
            clamp_column("ééé", 6),
            6,
            "the end of the line stays the end"
        );
    }

    #[test]
    fn saved_cursor_is_kept_inside_the_text() {
        let state = ScratchState {
            text: "one\ncafé".to_string(),
            view: ScratchView {
                cursor_line: 9,
                cursor_column: 99,
                scroll_line: None,
            },
        };
        assert_eq!(state.cursor(), (1, 5));
        let empty = ScratchState::default();
        assert_eq!(empty.cursor(), (0, 0));
    }
}
