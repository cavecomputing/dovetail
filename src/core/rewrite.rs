//! A rewrite's draft: a copy of a file's text that the window edits beside the original. It
//! autosaves to its own file in the data folder, so the original is only written when Ctrl+S
//! asks. The draft file lasting means a rewrite left open picks up again the next time its file
//! opens. Pausing a rewrite moves its draft aside, kept for the next Ctrl+R but not reopened with
//! the file. Scratch has no rewrite.

use std::fs;
use std::io::{self, ErrorKind};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use similar::{ChangeTag, TextDiff};

use crate::core::lock::file_lock_path;
use crate::core::save::atomic_write;

/// Where the draft of a rewrite of the file at `path` is kept: `drafts/` in the data folder,
/// named like the file's lock (from a hash of its real path).
pub fn draft_path(data_dir: &Path, path: &Path) -> PathBuf {
    let lock = file_lock_path(data_dir, path);
    let name = lock.file_stem().expect("a lock path ends in a file name");
    data_dir.join("drafts").join(name).with_extension("txt")
}

/// Where a paused rewrite's draft waits for the next Ctrl+R.
fn paused_path(data_dir: &Path, path: &Path) -> PathBuf {
    draft_path(data_dir, path).with_extension("paused")
}

/// The draft of a rewrite of the file at `path` left open, if there is one.
pub fn read_draft(data_dir: &Path, path: &Path) -> Option<String> {
    fs::read_to_string(draft_path(data_dir, path)).ok()
}

/// Starts a rewrite of the file at `path`: its draft begins as `text`.
pub fn write_draft(data_dir: &Path, path: &Path, text: &str) -> io::Result<()> {
    atomic_write(&draft_path(data_dir, path), text.as_bytes())
}

/// Pauses the rewrite of the file at `path`: its draft, `text`, is kept for the next Ctrl+R, but
/// no longer reopens with the file.
pub fn pause_draft(data_dir: &Path, path: &Path, text: &str) -> io::Result<()> {
    atomic_write(&paused_path(data_dir, path), text.as_bytes())?;
    remove_file(&draft_path(data_dir, path))
}

/// The draft a paused rewrite of the file at `path` left, if there is one, made the rewrite's
/// draft again.
pub fn take_paused(data_dir: &Path, path: &Path) -> Option<String> {
    let paused = paused_path(data_dir, path);
    let text = fs::read_to_string(&paused).ok()?;
    fs::rename(&paused, draft_path(data_dir, path)).ok()?;
    Some(text)
}

/// Deletes the rewrite of the file at `path`, open or paused.
pub fn remove_draft(data_dir: &Path, path: &Path) -> io::Result<()> {
    remove_file(&draft_path(data_dir, path))?;
    remove_file(&paused_path(data_dir, path))
}

/// What a draft changes from its original, word by word: the byte ranges of the original it takes
/// out, and of the draft it puts in.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Changes {
    pub removed: Vec<Range<usize>>,
    pub added: Vec<Range<usize>>,
}

/// How long working out `changes` may take, as it runs on every edit; past it, the diff is
/// rougher but still right.
const DIFF_TIME: Duration = Duration::from_millis(20);

/// What `draft` changes from `original`, word by word.
pub fn changes(original: &str, draft: &str) -> Changes {
    let diff = TextDiff::configure()
        .timeout(DIFF_TIME)
        .diff_words(original, draft);
    let mut changes = Changes::default();
    let (mut old, mut new) = (0, 0);
    for change in diff.iter_all_changes() {
        let length = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                old += length;
                new += length;
            }
            ChangeTag::Delete => {
                extend(&mut changes.removed, old..old + length, original);
                old += length;
            }
            ChangeTag::Insert => {
                extend(&mut changes.added, new..new + length, draft);
                new += length;
            }
        }
    }
    changes
}

/// Adds `range` of `text` to `ranges`, joined to the last one when only spaces lie between them,
/// so changed words side by side light up as one run rather than a row of patches.
fn extend(ranges: &mut Vec<Range<usize>>, range: Range<usize>, text: &str) {
    match ranges.last_mut() {
        Some(last) if text[last.end..range.start].bytes().all(|byte| byte == b' ') => {
            last.end = range.end;
        }
        _ => ranges.push(range),
    }
}

/// Deletes the file at `path`, if there is one.
fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_is_kept_until_the_rewrite_ends() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.md");
        assert_eq!(read_draft(dir.path(), &notes), None);
        write_draft(dir.path(), &notes, "draft").unwrap();
        assert_eq!(read_draft(dir.path(), &notes).as_deref(), Some("draft"));
        // Each file has its own.
        assert_eq!(read_draft(dir.path(), &dir.path().join("other.md")), None);
        remove_draft(dir.path(), &notes).unwrap();
        assert_eq!(read_draft(dir.path(), &notes), None);
        // Ending a rewrite with no draft left is fine too.
        remove_draft(dir.path(), &notes).unwrap();
    }

    #[test]
    fn changes_are_found_word_by_word() {
        let original = "Always write in a friendly tone.\nKeep it short.\n";
        let draft = "Write in a warm tone.\nKeep it short.\nThanks!\n";
        let found = changes(original, draft);
        let words = |text: &str, ranges: &[Range<usize>]| -> Vec<String> {
            ranges
                .iter()
                .map(|range| text[range.clone()].to_string())
                .collect()
        };
        assert_eq!(
            words(original, &found.removed),
            ["Always write", "friendly"]
        );
        assert_eq!(words(draft, &found.added), ["Write", "warm", "Thanks!\n"]);
        // Changed words with only a space between them are one run.
        let draft = "make each line short";
        let found = changes("keep every entry short", draft);
        assert_eq!(words(draft, &found.added), ["make each line"]);
        assert_eq!(changes(draft, draft), Changes::default());
    }

    #[test]
    fn a_paused_draft_waits_for_the_next_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.md");
        write_draft(dir.path(), &notes, "first").unwrap();
        pause_draft(dir.path(), &notes, "edited").unwrap();
        assert_eq!(
            read_draft(dir.path(), &notes),
            None,
            "not reopened with the file"
        );
        assert_eq!(take_paused(dir.path(), &notes).as_deref(), Some("edited"));
        assert_eq!(read_draft(dir.path(), &notes).as_deref(), Some("edited"));
        assert_eq!(take_paused(dir.path(), &notes), None);
        // Removing a rewrite takes a paused draft too.
        pause_draft(dir.path(), &notes, "edited").unwrap();
        remove_draft(dir.path(), &notes).unwrap();
        assert_eq!(take_paused(dir.path(), &notes), None);
    }
}
