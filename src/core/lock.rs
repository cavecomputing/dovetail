//! Locks that stop two windows from editing the same document.
//!
//! The lock is the operating system's own file lock, so it is released the moment the process
//! ends, even after a crash or a power cut, and it can never be left behind stale. A window
//! letting go of a document deletes its lock file too (Linux and macOS), so `locks/` holds only
//! the documents open now, give or take one left by a crash.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// A claim on a document: the lock on its lock file, held until this is dropped.
#[derive(Debug)]
pub struct Lock {
    _file: File,
    path: PathBuf,
}

impl Drop for Lock {
    /// Deletes the lock file while still holding it, so no other window can claim it in between;
    /// one that opened it just before sees it gone once it gets the lock, and starts again
    /// (`claim_lock`). Windows keeps the file: there a file another window has open can't be made
    /// again until that window closes it, which would leave the next window unguarded.
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = fs::remove_file(&self.path);
    }
}

/// Takes the lock at `lock_path`, creating the file and its folder if needed. Keep the returned
/// `Lock` for as long as the lock should be held; dropping it lets the lock go.
/// `Err(TryLockError::WouldBlock)` means another running process holds it.
pub fn claim_lock(lock_path: &Path) -> Result<Lock, TryLockError> {
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir).map_err(TryLockError::Error)?;
    }
    // A lock file deleted between opening and locking it (see `Lock`'s drop) guards nothing, so
    // start again on the new one. Three tries; a file system that loses files any faster than
    // that gets the lock it has.
    let mut tries = 0;
    loop {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)
            .map_err(TryLockError::Error)?;
        file.try_lock()?;
        tries += 1;
        if tries == 3 || still_there(&file, lock_path) {
            return Ok(Lock {
                _file: file,
                path: lock_path.to_path_buf(),
            });
        }
    }
}

/// True when `path` is still the file `file` opened, not deleted or made again since.
#[cfg(unix)]
fn still_there(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), fs::metadata(path)) {
        (Ok(held), Ok(now)) => held.dev() == now.dev() && held.ino() == now.ino(),
        _ => false,
    }
}

/// Windows never deletes a lock file (see `Lock`), so the one opened is always still there.
#[cfg(not(unix))]
fn still_there(_file: &File, _path: &Path) -> bool {
    true
}

/// True when another running process holds the lock at `lock_path`.
pub fn locked_elsewhere(lock_path: &Path) -> bool {
    matches!(claim_lock(lock_path), Err(TryLockError::WouldBlock))
}

/// Where the lock for the text file at `path` lives: one small file per document in the data
/// folder, named from a hash of the document's real path (a symlink and its target share one).
pub fn file_lock_path(data_dir: &Path, path: &Path) -> PathBuf {
    let real_path = match fs::canonicalize(path) {
        Ok(real_path) => real_path,
        // A file not made yet is named by its folder's real path, so the name stays the same once
        // it exists (Windows writes real paths as `\\?\C:\...`, so the plain path would differ).
        Err(_) => match (path.parent().map(fs::canonicalize), path.file_name()) {
            (Some(Ok(dir)), Some(name)) => dir.join(name),
            _ => path.to_path_buf(),
        },
    };
    let mut hasher = DefaultHasher::new();
    real_path.hash(&mut hasher);
    data_dir
        .join("locks")
        .join(format!("{:016x}.lock", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_claim_is_refused_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/doc.lock");
        let first = claim_lock(&path).unwrap();
        assert!(matches!(claim_lock(&path), Err(TryLockError::WouldBlock)));
        assert!(locked_elsewhere(&path));
        drop(first);
        assert!(!locked_elsewhere(&path));
        assert!(claim_lock(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn letting_go_deletes_the_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.lock");
        let lock = claim_lock(&path).unwrap();
        assert!(path.exists());
        drop(lock);
        assert!(!path.exists());
        // A check that finds nobody holding it leaves none behind either.
        assert!(!locked_elsewhere(&path));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_file_deleted_before_it_was_locked_is_claimed_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.lock");
        // Opened before another window deleted it, as a claim in between would have.
        let stale = File::create(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!still_there(&stale, &path));
        let lock = claim_lock(&path).unwrap();
        assert!(still_there(&lock._file, &path));
    }

    #[test]
    fn a_file_keeps_its_lock_once_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let before = file_lock_path(dir.path(), &path);
        fs::write(&path, "x").unwrap();
        assert_eq!(file_lock_path(dir.path(), &path), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_and_its_target_share_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "x").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            file_lock_path(dir.path(), &target),
            file_lock_path(dir.path(), &link)
        );
        assert_ne!(
            file_lock_path(dir.path(), &target),
            file_lock_path(dir.path(), &dir.path().join("other.txt"))
        );
    }
}
