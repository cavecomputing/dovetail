//! The recently opened files list, kept in the data folder as one path per line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::core::save::atomic_write;

const RECENT_FILE: &str = "recent.txt";
const MAX_RECENT: usize = 10;

/// The recent files, newest first. A missing or unreadable list is just empty.
pub fn recent_files(data_dir: &Path) -> Vec<PathBuf> {
    let text = fs::read_to_string(data_dir.join(RECENT_FILE)).unwrap_or_default();
    text.lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Moves `path` to the top of the list (adding it if new) and saves the list.
pub fn remember(data_dir: &Path, path: &Path) -> io::Result<()> {
    let mut list = recent_files(data_dir);
    list.retain(|old| old != path);
    list.insert(0, path.to_path_buf());
    list.truncate(MAX_RECENT);
    let mut text = String::new();
    for entry in &list {
        text.push_str(&entry.to_string_lossy());
        text.push('\n');
    }
    atomic_write(&data_dir.join(RECENT_FILE), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_when_nothing_saved() {
        let dir = tempfile::tempdir().unwrap();
        assert!(recent_files(dir.path()).is_empty());
    }

    #[test]
    fn newest_first_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        remember(dir.path(), Path::new("/a.txt")).unwrap();
        remember(dir.path(), Path::new("/b.txt")).unwrap();
        remember(dir.path(), Path::new("/a.txt")).unwrap();
        assert_eq!(
            recent_files(dir.path()),
            vec![PathBuf::from("/a.txt"), PathBuf::from("/b.txt")]
        );
    }

    #[test]
    fn keeps_only_the_latest_ten() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..15 {
            remember(dir.path(), Path::new(&format!("/{n}.txt"))).unwrap();
        }
        let list = recent_files(dir.path());
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0], PathBuf::from("/14.txt"));
    }
}
