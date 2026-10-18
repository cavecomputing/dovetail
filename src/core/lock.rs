//! Locks that stop two windows from editing the same document.
//!
//! The lock is the operating system's own file lock, so it is released the moment the process
//! ends, even after a crash or a power cut, and it can never be left behind stale.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// Takes the lock at `lock_path`, creating the file and its folder if needed. Keep the returned
/// `File` for as long as the lock should be held; dropping it lets the lock go.
/// `Err(TryLockError::WouldBlock)` means another running process holds it.
pub fn claim_lock(lock_path: &Path) -> Result<File, TryLockError> {
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir).map_err(TryLockError::Error)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
        .map_err(TryLockError::Error)?;
    file.try_lock()?;
    Ok(file)
}

/// True when another running process holds the lock at `lock_path`.
pub fn locked_elsewhere(lock_path: &Path) -> bool {
    matches!(claim_lock(lock_path), Err(TryLockError::WouldBlock))
}

/// Where the lock for the text file at `path` lives: one small file per document in the data
/// folder, named from a hash of the document's real path (a symlink and its target share one).
pub fn file_lock_path(data_dir: &Path, path: &Path) -> PathBuf {
    let real_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
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
