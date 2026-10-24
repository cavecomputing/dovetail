//! A rewrite's draft: a copy of a file's text that the window edits beside the original. It
//! autosaves to its own file in the data folder, so the original is only written when Ctrl+S
//! asks. The draft file lasting means a rewrite left open picks up again the next time its file
//! opens; ending the rewrite deletes it. Scratch has no rewrite.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::core::lock::file_lock_path;
use crate::core::save::atomic_write;

/// Where the draft of a rewrite of the file at `path` is kept: `drafts/` in the data folder,
/// named like the file's lock (from a hash of its real path).
pub fn draft_path(data_dir: &Path, path: &Path) -> PathBuf {
    let lock = file_lock_path(data_dir, path);
    let name = lock.file_stem().expect("a lock path ends in a file name");
    data_dir.join("drafts").join(name).with_extension("txt")
}

/// The draft of a rewrite of the file at `path` left open, if there is one.
pub fn read_draft(data_dir: &Path, path: &Path) -> Option<String> {
    fs::read_to_string(draft_path(data_dir, path)).ok()
}

/// Starts a rewrite of the file at `path`: its draft begins as `text`.
pub fn write_draft(data_dir: &Path, path: &Path, text: &str) -> io::Result<()> {
    atomic_write(&draft_path(data_dir, path), text.as_bytes())
}

/// Ends the rewrite of the file at `path`: its draft is deleted.
pub fn remove_draft(data_dir: &Path, path: &Path) -> io::Result<()> {
    match fs::remove_file(draft_path(data_dir, path)) {
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
}
