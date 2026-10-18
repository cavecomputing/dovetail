//! The recently opened files list, kept in the data folder as one path per line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::brand::PROJECT_EXT;
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

/// The recent files that still exist, newest first, for the menu and the switcher: a file
/// deleted or moved since is left out, and so is a project manifest from the rewrite branch.
pub fn existing_recent_files(data_dir: &Path) -> Vec<PathBuf> {
    recent_files(data_dir)
        .into_iter()
        .filter(|path| path.exists())
        .filter(|path| path.extension().is_none_or(|ext| ext != PROJECT_EXT))
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

/// How well `query` fuzzy-matches `label`, higher is better; `None` if it doesn't match at all.
/// Every query character must appear in `label` in order (ignoring case). Characters that follow
/// one another, or start a word, score more, so "nyx" ranks "Nyx" above "a noisy x".
pub fn match_score(query: &str, label: &str) -> Option<i32> {
    let label: Vec<char> = label.to_lowercase().chars().collect();
    let mut score = 0;
    let mut from = 0;
    let mut previous: Option<usize> = None;
    for wanted in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = from + label[from..].iter().position(|&c| c == wanted)?;
        score += 1;
        if previous == Some(found.wrapping_sub(1)) {
            score += 4;
        }
        if found == 0 || !label[found - 1].is_alphanumeric() {
            score += 3;
        }
        previous = Some(found);
        from = found + 1;
    }
    Some(score)
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

    #[test]
    fn existing_files_leave_out_the_ones_that_are_gone_and_projects() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.txt");
        fs::write(&notes, "hi").unwrap();
        let manifest = dir.path().join(format!("Nyx.{PROJECT_EXT}"));
        fs::write(&manifest, "{}").unwrap();
        remember(dir.path(), &dir.path().join("gone.txt")).unwrap();
        remember(dir.path(), &manifest).unwrap();
        remember(dir.path(), &notes).unwrap();
        assert_eq!(recent_files(dir.path()).len(), 3);
        assert_eq!(existing_recent_files(dir.path()), vec![notes]);
    }

    #[test]
    fn fuzzy_matching_needs_the_letters_in_order() {
        assert!(match_score("nyx", "Nyx is a").is_some());
        assert!(match_score("NX", "nyx").is_some());
        assert!(match_score("xn", "nyx").is_none());
        assert_eq!(match_score("", "anything"), Some(0));
        assert!(match_score("nyx", "Nyx").unwrap() > match_score("nyx", "a noisy x").unwrap());
    }
}
