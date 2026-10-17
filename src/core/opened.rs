//! What launching on a document means: which text to show, where the cursor goes, and whether
//! the window may edit it (and if not, why not).

use std::fs::{File, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::core::lock::{claim_lock, file_lock_path};
use crate::core::paths::Paths;
use crate::core::recent::remember;
use crate::core::save::{atomic_write, check_writable};
use crate::core::scratch::{ScratchState, claim_scratch, take_cleared};
use crate::core::text_file::{DiskStamp, LineEnding, read_text};
use crate::project::DRAFT_FILE;

/// What a window is showing.
#[derive(Debug, Clone)]
pub enum Doc {
    Scratch,
    File {
        path: PathBuf,
        line_ending: LineEnding,
    },
    /// A project, by its manifest's path. The window edits its `draft.txt`.
    Project(PathBuf),
}

/// A document ready for the window.
#[derive(Debug)]
pub struct OpenedDoc {
    pub doc: Doc,
    pub text: String,
    /// Where to put the cursor, as (line, byte column); `None` leaves it at the start.
    pub cursor: Option<(usize, usize)>,
    /// The line to show at the top of the view; `None` leaves the view around the cursor.
    pub scroll_line: Option<usize>,
    /// Why the text can't be edited; `None` means it can.
    pub read_only_reason: Option<String>,
    /// The lock that keeps other windows off this document. Hold it while the window is open.
    pub lock: Option<File>,
    /// The file on disk as it was read, so a save can tell if someone else changed it.
    pub disk_stamp: Option<DiskStamp>,
    /// What a Rewrite cleared from scratch, for the window to offer as its first undo step.
    pub cleared_scratch: Option<String>,
}

/// Opens `doc`: claims it so no second window edits it, reads it, and decides whether it can be
/// edited. A `.txt` that doesn't exist yet is created empty.
pub fn open_doc(doc: Doc, paths: &Paths) -> OpenedDoc {
    let mut opened = OpenedDoc {
        doc: doc.clone(),
        text: String::new(),
        cursor: None,
        scroll_line: None,
        read_only_reason: None,
        lock: None,
        disk_stamp: None,
        cleared_scratch: None,
    };
    match doc {
        Doc::Scratch => {
            let claim = claim_scratch(&paths.data_dir);
            match ScratchState::load(&paths.data_dir) {
                Ok(scratch) => {
                    opened.text = scratch.text.clone();
                    opened.cursor = Some(scratch.cursor());
                    opened.scroll_line = scratch.view.scroll_line;
                }
                Err(error) => {
                    opened.read_only_reason = Some(format!("Couldn't read scratch: {error}"));
                }
            }
            match claim {
                Err(TryLockError::WouldBlock) => {
                    opened.read_only_reason = Some("Scratch is open in another window".to_string());
                }
                // Without a lock file there is nothing to guard with, so carry on.
                Err(TryLockError::Error(_)) => {}
                Ok(lock) => {
                    opened.lock = Some(lock);
                    opened.cleared_scratch = take_cleared(&paths.data_dir);
                }
            }
        }
        Doc::File { path, .. } => {
            let _ = remember(&paths.data_dir, &path);
            let mut line_ending = LineEnding::default();
            match read_text(&path) {
                Ok(file) => {
                    opened.text = file.text;
                    line_ending = file.line_ending;
                    opened.read_only_reason = writable_reason(&path);
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    if atomic_write(&path, b"").is_err() {
                        opened.read_only_reason =
                            Some("Read-only: no permission to save here".to_string());
                    }
                }
                Err(error) => opened.read_only_reason = Some(format!("Couldn't open: {error}")),
            }
            opened.disk_stamp = DiskStamp::of(&path);
            opened.doc = Doc::File { path, line_ending };
        }
        Doc::Project(path) => {
            let _ = remember(&paths.data_dir, &path);
            match read_text(&path.with_file_name(DRAFT_FILE)) {
                Ok(file) => opened.text = file.text,
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => opened.read_only_reason = Some(format!("Couldn't open: {error}")),
            }
        }
    }
    if let Doc::File { path, .. } | Doc::Project(path) = &opened.doc
        && opened.read_only_reason.is_none()
    {
        opened.lock = match claim_lock(&file_lock_path(&paths.data_dir, path)) {
            Err(TryLockError::WouldBlock) => {
                opened.read_only_reason = Some("Open in another window".to_string());
                None
            }
            Err(TryLockError::Error(_)) => None,
            Ok(lock) => Some(lock),
        };
    }
    opened
}

/// Why `path` can't be saved, or `None` if it can.
fn writable_reason(path: &Path) -> Option<String> {
    match check_writable(path) {
        Ok(()) => None,
        Err(_) => Some("Read-only: no permission to save here".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn file_doc(path: &Path) -> Doc {
        Doc::File {
            path: path.to_path_buf(),
            line_ending: LineEnding::default(),
        }
    }

    #[test]
    fn scratch_resumes_with_its_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        fs::create_dir_all(&paths.data_dir).unwrap();
        fs::write(paths.data_dir.join("scratch.txt"), "café").unwrap();
        fs::write(
            paths.data_dir.join("scratch.json"),
            r#"{"cursor_line":0,"cursor_column":4}"#,
        )
        .unwrap();
        let opened = open_doc(Doc::Scratch, &paths);
        assert_eq!(opened.text, "café");
        assert!(opened.read_only_reason.is_none());
        // Byte 4 is inside "é"; it's moved back to the start of the character.
        assert_eq!(opened.cursor, Some((0, 3)));
        assert_eq!(
            opened.scroll_line, None,
            "an older scratch.json has no scroll line"
        );
    }

    #[test]
    fn second_scratch_window_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let first = open_doc(Doc::Scratch, &paths);
        assert!(first.read_only_reason.is_none());
        let second = open_doc(Doc::Scratch, &paths);
        assert_eq!(
            second.read_only_reason.as_deref(),
            Some("Scratch is open in another window")
        );
        drop(first);
        assert!(open_doc(Doc::Scratch, &paths).read_only_reason.is_none());
    }

    #[test]
    fn missing_text_file_is_created_and_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = dir.path().join("new.txt");
        let opened = open_doc(file_doc(&path), &paths);
        assert!(opened.read_only_reason.is_none());
        assert!(path.exists());
        assert!(opened.disk_stamp.is_some());
        assert_eq!(
            crate::core::recent::recent_files(&paths.data_dir),
            vec![path]
        );
    }

    #[test]
    fn second_window_on_a_file_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = dir.path().join("notes.txt");
        fs::write(&path, "hi").unwrap();
        let first = open_doc(file_doc(&path), &paths);
        assert!(first.read_only_reason.is_none());
        let second = open_doc(file_doc(&path), &paths);
        assert_eq!(second.text, "hi");
        assert_eq!(
            second.read_only_reason.as_deref(),
            Some("Open in another window")
        );
    }

    #[test]
    fn unreadable_file_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = dir.path().join("binary.txt");
        fs::write(&path, [0xff, 0xfe]).unwrap();
        let opened = open_doc(file_doc(&path), &paths);
        assert_eq!(
            opened.read_only_reason.as_deref(),
            Some("Couldn't open: not a UTF-8 text file")
        );
    }

    #[test]
    fn file_in_a_folder_that_cant_be_written_is_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(&dir.path().join("home"));
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("notes.txt"), "hi").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores permissions, so only check when they apply.
        if fs::write(locked.join("probe"), "").is_err() {
            for name in ["notes.txt", "missing.txt"] {
                let opened = open_doc(file_doc(&locked.join(name)), &paths);
                assert_eq!(
                    opened.read_only_reason.as_deref(),
                    Some("Read-only: no permission to save here")
                );
            }
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn project_opens_its_draft_and_is_locked_to_one_window() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let project = crate::project::create_project(&paths.projects_dir, "Nyx").unwrap();
        atomic_write(&project.draft_path(), b"Nyx, rewritten").unwrap();
        let first = open_doc(Doc::Project(project.manifest_path()), &paths);
        assert_eq!(first.text, "Nyx, rewritten");
        assert!(first.read_only_reason.is_none());
        let second = open_doc(Doc::Project(project.manifest_path()), &paths);
        assert_eq!(
            second.read_only_reason.as_deref(),
            Some("Open in another window")
        );
        assert_eq!(
            crate::core::recent::recent_files(&paths.data_dir),
            vec![project.manifest_path()]
        );
    }

    #[test]
    fn scratch_offers_what_a_rewrite_cleared_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        crate::core::scratch::clear_scratch(&paths.data_dir, "old").unwrap();
        let first = open_doc(Doc::Scratch, &paths);
        assert_eq!(first.text, "");
        assert_eq!(first.cleared_scratch.as_deref(), Some("old"));
        drop(first);
        assert_eq!(open_doc(Doc::Scratch, &paths).cleared_scratch, None);
    }
}
