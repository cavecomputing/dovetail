//! Atomic file writes and the autosave state machine.
//!
//! Every save goes to a temp file in the same folder, is flushed to disk, then renamed over the
//! target. A rename within one folder is atomic, so a crash mid-save leaves either the old file or
//! the new one, never half of each.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Numbers each temp file, so two saves running at once never write into the same one.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` atomically, creating the parent folder if needed. An existing file
/// keeps its permissions. A symlink is followed, so the link stays a link and its target gets the
/// new text.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (path, dir, temp) = save_paths(path)?;
    fs::create_dir_all(&dir)?;

    let result = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        if let Ok(old) = fs::metadata(&path) {
            file.set_permissions(old.permissions())?;
        }
        file.sync_all()?;
        fs::rename(&temp, &path)?;
        // Flush the folder entry too, so the rename itself survives a power cut.
        File::open(&dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Checks that `path` could be saved, by making and removing the temp file a save writes first.
/// Saving needs the folder to be writable, since the temp file is renamed over the target.
pub fn check_writable(path: &Path) -> io::Result<()> {
    let (_, _, temp) = save_paths(path)?;
    File::create(&temp)?;
    fs::remove_file(&temp)
}

/// The file a save really replaces (a symlink resolved to its target), its folder, and the temp
/// file written there first.
fn save_paths(path: &Path) -> io::Result<(PathBuf, PathBuf, PathBuf)> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("no file name"))?;
    let temp = dir.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    Ok((path, dir, temp))
}

/// What the save dot in the status bar shows.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveState {
    Saved,
    Saving,
    /// A short human sentence such as "Save failed: disk full".
    Failed(String),
}

/// Turns an I/O error into the sentence the status bar shows.
pub fn failure_reason(error: &io::Error) -> String {
    let words = match error.kind() {
        ErrorKind::StorageFull => "disk full".to_string(),
        ErrorKind::QuotaExceeded => "over disk quota".to_string(),
        ErrorKind::PermissionDenied => "no permission".to_string(),
        ErrorKind::ReadOnlyFilesystem => "read-only disk".to_string(),
        ErrorKind::NotFound => "folder missing".to_string(),
        _ => error.to_string(),
    };
    format!("Save failed: {words}")
}

/// Decides when to autosave and whether the window may close.
///
/// Every edit bumps a generation number. A save records which generation it wrote, so an edit
/// made while a save is running is still saved afterwards. Time is passed in so tests can control
/// it.
#[derive(Debug)]
pub struct Autosaver {
    delay: Duration,
    edited_generation: u64,
    saved_generation: u64,
    saving_generation: Option<u64>,
    /// The last edit or save attempt; saving waits `delay` after it.
    wait_from: Option<Instant>,
    failure: Option<String>,
}

impl Autosaver {
    /// `delay` is how long typing must pause before a save starts.
    pub fn new(delay: Duration) -> Autosaver {
        Autosaver {
            delay,
            edited_generation: 0,
            saved_generation: 0,
            saving_generation: None,
            wait_from: None,
            failure: None,
        }
    }

    pub fn set_delay(&mut self, delay: Duration) {
        self.delay = delay;
    }

    /// Call on every change to the text.
    pub fn edited(&mut self, now: Instant) {
        self.edited_generation += 1;
        self.wait_from = Some(now);
    }

    /// True when there are changes on screen that are not on disk yet.
    pub fn pending(&self) -> bool {
        self.edited_generation > self.saved_generation
    }

    /// True while a save is being written.
    pub fn is_saving(&self) -> bool {
        self.saving_generation.is_some()
    }

    /// True when a save should start: changes are pending, typing has paused for the delay, and no
    /// save is already running. A failed save is retried this way after each delay.
    pub fn should_save_now(&self, now: Instant) -> bool {
        let paused = match self.wait_from {
            Some(last) => now.duration_since(last) >= self.delay,
            None => true,
        };
        self.pending() && paused && self.saving_generation.is_none()
    }

    /// Call when a save starts. Returns the generation being written; hand it back to `finish`.
    /// The next attempt (a retry after a failure) waits a full delay from `now`.
    pub fn begin(&mut self, now: Instant) -> u64 {
        self.saving_generation = Some(self.edited_generation);
        self.wait_from = Some(now);
        self.edited_generation
    }

    /// Call when a save ends, with the generation from `begin` and the outcome (the error being
    /// the sentence from `failure_reason`).
    pub fn finish(&mut self, generation: u64, result: Result<(), String>) {
        self.saving_generation = None;
        match result {
            Ok(()) => {
                self.saved_generation = self.saved_generation.max(generation);
                self.failure = None;
            }
            Err(reason) => self.failure = Some(reason),
        }
    }

    /// Call when the text on screen is known to match the disk without this saver writing it,
    /// such as after reloading the file or when a commit wrote it. A save still running stays
    /// running until `finish`, so a second one can't start beside it.
    pub fn mark_saved(&mut self) {
        self.saved_generation = self.edited_generation;
        self.failure = None;
    }

    /// True only when everything is on disk and the last save worked.
    pub fn close_allowed(&self) -> bool {
        !self.pending() && self.saving_generation.is_none() && self.failure.is_none()
    }

    /// What the save dot shows. Changes waiting for the delay count as saving, since there is no
    /// "unsaved" state.
    pub fn state(&self) -> SaveState {
        if let Some(reason) = &self.failure {
            return SaveState::Failed(reason.clone());
        }
        if self.pending() || self.saving_generation.is_some() {
            SaveState::Saving
        } else {
            SaveState::Saved
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: Duration = Duration::from_millis(500);

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/notes.txt");
        atomic_write(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        // No temp file left behind.
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn atomic_write_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.txt");
        fs::write(&path, b"x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        atomic_write(&path, b"y").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn atomic_write_fails_cleanly_into_a_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        fs::write(&blocker, b"").unwrap();
        assert!(atomic_write(&blocker.join("child.txt"), b"x").is_err());
    }

    #[test]
    fn atomic_write_through_a_symlink_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, b"old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        atomic_write(&link, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    }

    #[test]
    fn check_writable_needs_a_writable_folder() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        check_writable(&path).unwrap();
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            0,
            "probe removed"
        );
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores permissions, so only check the refusal when it applies.
        if File::create(dir.path().join("probe")).is_err() {
            assert!(check_writable(&path).is_err());
        }
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn failure_reason_uses_short_words() {
        let full = io::Error::from(ErrorKind::StorageFull);
        assert_eq!(failure_reason(&full), "Save failed: disk full");
        let denied = io::Error::from(ErrorKind::PermissionDenied);
        assert_eq!(failure_reason(&denied), "Save failed: no permission");
        let other = io::Error::other("strange");
        assert_eq!(failure_reason(&other), "Save failed: strange");
    }

    #[test]
    fn autosaver_starts_saved_and_closable() {
        let saver = Autosaver::new(DELAY);
        assert_eq!(saver.state(), SaveState::Saved);
        assert!(saver.close_allowed());
        assert!(!saver.should_save_now(Instant::now()));
    }

    #[test]
    fn autosave_waits_for_the_delay() {
        let start = Instant::now();
        let mut saver = Autosaver::new(DELAY);
        saver.edited(start);
        assert_eq!(saver.state(), SaveState::Saving);
        assert!(!saver.close_allowed());
        assert!(!saver.should_save_now(start + Duration::from_millis(100)));
        assert!(saver.should_save_now(start + DELAY));
    }

    #[test]
    fn autosave_round_trip() {
        let start = Instant::now();
        let mut saver = Autosaver::new(DELAY);
        saver.edited(start);
        let generation = saver.begin(start + DELAY);
        assert!(
            !saver.should_save_now(start + DELAY * 3),
            "one save at a time"
        );
        saver.finish(generation, Ok(()));
        assert_eq!(saver.state(), SaveState::Saved);
        assert!(saver.close_allowed());
    }

    #[test]
    fn edit_during_save_is_saved_after() {
        let start = Instant::now();
        let mut saver = Autosaver::new(DELAY);
        saver.edited(start);
        let first = saver.begin(start);
        saver.edited(start + Duration::from_millis(10));
        saver.finish(first, Ok(()));
        assert!(saver.pending());
        assert!(!saver.close_allowed());
        assert!(saver.should_save_now(start + Duration::from_secs(1)));
        let second = saver.begin(start + Duration::from_secs(1));
        saver.finish(second, Ok(()));
        assert!(saver.close_allowed());
    }

    #[test]
    fn failed_save_blocks_close_and_retries() {
        let start = Instant::now();
        let mut saver = Autosaver::new(DELAY);
        saver.edited(start);
        let generation = saver.begin(start + DELAY);
        saver.finish(generation, Err("Save failed: disk full".into()));
        assert_eq!(
            saver.state(),
            SaveState::Failed("Save failed: disk full".into())
        );
        assert!(!saver.close_allowed());
        assert!(
            !saver.should_save_now(start + DELAY),
            "no instant retry loop"
        );
        assert!(
            saver.should_save_now(start + DELAY * 2),
            "retries after the delay"
        );
        let retry = saver.begin(start + DELAY * 2);
        saver.finish(retry, Ok(()));
        assert_eq!(saver.state(), SaveState::Saved);
        assert!(saver.close_allowed());
    }

    #[test]
    fn mark_saved_clears_a_failure_and_pending_edits() {
        let start = Instant::now();
        let mut saver = Autosaver::new(DELAY);
        saver.edited(start);
        let generation = saver.begin(start + DELAY);
        saver.finish(generation, Err("Save failed: changed on disk".into()));
        saver.mark_saved();
        assert_eq!(saver.state(), SaveState::Saved);
        assert!(saver.close_allowed());
        assert!(!saver.should_save_now(start + DELAY * 3));
    }

    #[test]
    fn mark_saved_leaves_a_running_save_running() {
        let start = Instant::now();
        let mut saver = Autosaver::new(DELAY);
        saver.edited(start);
        let generation = saver.begin(start + DELAY);
        saver.edited(start + DELAY);
        saver.mark_saved();
        assert!(saver.is_saving());
        assert!(
            !saver.should_save_now(start + DELAY * 3),
            "one save at a time"
        );
        saver.finish(generation, Ok(()));
        assert!(saver.close_allowed());
    }

    #[test]
    fn each_save_gets_its_own_temp_file() {
        let (_, _, first) = save_paths(Path::new("/tmp/a.txt")).unwrap();
        let (_, _, second) = save_paths(Path::new("/tmp/a.txt")).unwrap();
        assert_ne!(first, second);
    }
}
