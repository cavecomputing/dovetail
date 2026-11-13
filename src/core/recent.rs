//! The recently opened files list, kept in the data folder as one path per line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::save::atomic_write;
use crate::core::scratch::ScratchView;

const RECENT_FILE: &str = "recent.txt";
const MAX_RECENT: usize = 10;
const PLACES_FILE: &str = "places.json";
const MAX_PLACES: usize = 50;

/// Where the cursor and the view were when a file was last left.
#[derive(Serialize, Deserialize)]
struct Place {
    path: PathBuf,
    view: ScratchView,
}

/// The saved places, newest first. A missing or broken file is just empty.
fn places(data_dir: &Path) -> Vec<Place> {
    fs::read_to_string(data_dir.join(PLACES_FILE))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// A file's name for the places list, the same however it is spelled.
fn place_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Where the cursor and the view were when `path` was last left, if that was kept.
pub fn saved_view(data_dir: &Path, path: &Path) -> Option<ScratchView> {
    let key = place_key(path);
    places(data_dir)
        .into_iter()
        .find(|place| place.path == key)
        .map(|place| place.view)
}

/// Keeps `view` as where `path` was left, for the next time it opens; the latest 50 files are kept.
pub fn remember_view(data_dir: &Path, path: &Path, view: ScratchView) -> io::Result<()> {
    let key = place_key(path);
    let mut list = places(data_dir);
    list.retain(|place| place.path != key);
    list.insert(0, Place { path: key, view });
    list.truncate(MAX_PLACES);
    let json = serde_json::to_string(&list).map_err(io::Error::other)?;
    atomic_write(&data_dir.join(PLACES_FILE), json.as_bytes())
}

/// The recent files, newest first. A missing or unreadable list is just empty.
pub fn recent_files(data_dir: &Path) -> Vec<PathBuf> {
    let text = fs::read_to_string(data_dir.join(RECENT_FILE)).unwrap_or_default();
    text.lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The recent files that still exist, newest first, for the menu and the switcher: a file
/// deleted or moved since is left out.
pub fn existing_recent_files(data_dir: &Path) -> Vec<PathBuf> {
    recent_files(data_dir)
        .into_iter()
        // Files only: a folder left from when folders could be opened is skipped.
        .filter(|path| path.is_file())
        .collect()
}

/// Moves `path` to the top of the list (adding it if new) and saves the list.
pub fn remember(data_dir: &Path, path: &Path) -> io::Result<()> {
    let mut list = recent_files(data_dir);
    list.retain(|old| !same_file(old, path));
    list.insert(0, path.to_path_buf());
    list.truncate(MAX_RECENT);
    let mut text = String::new();
    for entry in &list {
        text.push_str(&entry.to_string_lossy());
        text.push('\n');
    }
    atomic_write(&data_dir.join(RECENT_FILE), text.as_bytes())
}

/// Whether `a` and `b` are the same file on disk however each is spelled (through links, `..`,
/// or other letter case where the disk ignores case), or the same path when one is missing.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real(a) == real(b)
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
    fn a_file_comes_back_to_where_it_was_left() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.txt");
        fs::write(&notes, "hi").unwrap();
        assert_eq!(saved_view(dir.path(), &notes), None);
        let view = ScratchView {
            cursor_line: 3,
            cursor_column: 2,
            scroll_line: Some(1),
        };
        remember_view(dir.path(), &notes, view).unwrap();
        let again = ScratchView {
            cursor_line: 9,
            ..view
        };
        remember_view(dir.path(), &notes, again).unwrap();
        assert_eq!(saved_view(dir.path(), &notes), Some(again));
        // Spelled another way, it is the same file.
        let other_spelling = dir.path().join(".").join("notes.txt");
        assert_eq!(saved_view(dir.path(), &other_spelling), Some(again));
    }

    #[test]
    fn keeps_the_places_of_only_the_latest_fifty_files() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..60 {
            let path = PathBuf::from(format!("/{n}.txt"));
            remember_view(dir.path(), &path, ScratchView::default()).unwrap();
        }
        assert_eq!(places(dir.path()).len(), MAX_PLACES);
        assert!(saved_view(dir.path(), Path::new("/59.txt")).is_some());
        assert!(saved_view(dir.path(), Path::new("/0.txt")).is_none());
    }

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
    fn one_file_is_one_entry_however_it_is_spelled() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.txt");
        fs::write(&notes, "hi").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let other_spelling = dir.path().join("sub").join("..").join("notes.txt");
        remember(dir.path(), &notes).unwrap();
        remember(dir.path(), &other_spelling).unwrap();
        assert_eq!(recent_files(dir.path()), vec![other_spelling]);
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
    fn existing_files_leave_out_the_ones_that_are_gone() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.txt");
        fs::write(&notes, "hi").unwrap();
        remember(dir.path(), &dir.path().join("gone.txt")).unwrap();
        remember(dir.path(), &notes).unwrap();
        assert_eq!(recent_files(dir.path()).len(), 2);
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
