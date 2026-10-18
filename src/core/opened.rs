//! What launching on a document means: which text to show, where the cursor goes, and whether
//! the window may edit it (and if not, why not).

use std::fs::{self, File, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::core::lock::{claim_lock, file_lock_path};
use crate::core::paths::Paths;
use crate::core::recent::remember;
use crate::core::save::{atomic_write, check_writable, failure_reason};
use crate::core::scratch::{
    ScratchState, claim_scratch, scratch_lock_path, scratch_text_path, take_cleared,
};
use crate::core::text_file::{DiskStamp, LineEnding, read_text};
use crate::project::{DRAFT_FILE, is_project};

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
        opened.lock = match claim_lock(&lock_for(&paths.data_dir, path)) {
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

/// Gives a window's text a new home at `path` (Save as): claims the file so no other window
/// edits it, writes `text` there with `line_ending`, and remembers it as recent. Returns the
/// claim to hold while the window lives and the file as written, or why nothing was written.
pub fn save_as(
    data_dir: &Path,
    path: &Path,
    text: &str,
    line_ending: LineEnding,
) -> Result<(Option<File>, Option<DiskStamp>), String> {
    let lock = match claim_lock(&lock_for(data_dir, path)) {
        Err(TryLockError::WouldBlock) => return Err(OPEN_ELSEWHERE.to_string()),
        // Without a lock file there is nothing to guard with, so carry on.
        Err(TryLockError::Error(_)) => None,
        Ok(lock) => Some(lock),
    };
    atomic_write(path, line_ending.apply(text).as_bytes())
        .map_err(|error| failure_reason(&error))?;
    let _ = remember(data_dir, path);
    Ok((lock, DiskStamp::of(path)))
}

/// Empties `path` for Ctrl+N, when the save dialog has agreed to replace a file there, unless
/// another window has it open.
pub fn empty_file(data_dir: &Path, path: &Path) -> Result<(), String> {
    match claim_lock(&lock_for(data_dir, path)) {
        Err(TryLockError::WouldBlock) => Err(OPEN_ELSEWHERE.to_string()),
        _ => atomic_write(path, b"").map_err(|error| failure_reason(&error)),
    }
}

const OPEN_ELSEWHERE: &str = "That file is open in another window";

/// The lock that guards writing `path`. A project's draft is guarded by its project's lock and
/// scratch's text by scratch's own, so no window can open or save over either while it's open.
fn lock_for(data_dir: &Path, path: &Path) -> PathBuf {
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if real(path) == real(&scratch_text_path(data_dir)) {
        return scratch_lock_path(data_dir);
    }
    let manifest = match path.file_name() {
        Some(name) if name == DRAFT_FILE => manifest_beside(path),
        _ => None,
    };
    file_lock_path(data_dir, manifest.as_deref().unwrap_or(path))
}

/// The project manifest in the same folder as `path`, if there is one.
fn manifest_beside(path: &Path) -> Option<PathBuf> {
    fs::read_dir(path.parent()?)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| is_project(path))
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
    fn save_as_writes_the_text_and_claims_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = dir.path().join("notes.txt");
        let (lock, stamp) = save_as(&paths.data_dir, &path, "a\nb", LineEnding::CrLf).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\r\nb");
        assert!(lock.is_some() && stamp.is_some());
        // Another window opening it now gets it read-only.
        let other = open_doc(file_doc(&path), &paths);
        assert_eq!(
            other.read_only_reason.as_deref(),
            Some("Open in another window")
        );
    }

    #[test]
    fn save_as_refuses_a_file_open_in_another_window() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = dir.path().join("open.txt");
        fs::write(&path, "theirs").unwrap();
        let _other = open_doc(file_doc(&path), &paths);
        let refused = save_as(&paths.data_dir, &path, "mine", LineEnding::Lf);
        assert_eq!(refused.unwrap_err(), "That file is open in another window");
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
    }

    #[test]
    fn a_project_draft_and_scratch_text_are_guarded_by_their_windows() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let project = crate::project::create_project(&paths.projects_dir, "Nyx").unwrap();
        atomic_write(&project.draft_path(), b"draft").unwrap();
        let _project_window = open_doc(Doc::Project(project.manifest_path()), &paths);
        let _scratch_window = open_doc(Doc::Scratch, &paths);
        let scratch_text = scratch_text_path(&paths.data_dir);
        for path in [project.draft_path(), scratch_text.clone()] {
            let refused = save_as(&paths.data_dir, &path, "mine", LineEnding::Lf);
            assert_eq!(refused.unwrap_err(), OPEN_ELSEWHERE);
            assert_eq!(
                empty_file(&paths.data_dir, &path).unwrap_err(),
                OPEN_ELSEWHERE
            );
            let other = open_doc(file_doc(&path), &paths);
            assert_eq!(
                other.read_only_reason.as_deref(),
                Some("Open in another window")
            );
        }
        assert_eq!(fs::read_to_string(project.draft_path()).unwrap(), "draft");
    }

    #[test]
    fn empty_file_empties_a_file_no_window_has_open() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = dir.path().join("old.txt");
        fs::write(&path, "old text").unwrap();
        empty_file(&paths.data_dir, &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
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
