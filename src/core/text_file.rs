//! Reading plain `.txt` documents.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::time::SystemTime;

use crate::core::save::atomic_write;

/// The line ending a file used, so saving writes it back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    /// The most common ending in `text`; LF when there are no line breaks.
    pub fn detect(text: &str) -> LineEnding {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        if crlf > lf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }

    /// Converts editor text (always `\n`) to this ending for writing to disk.
    pub fn apply(self, text: &str) -> String {
        match self {
            LineEnding::Lf => text.to_string(),
            LineEnding::CrLf => text.replace('\n', "\r\n"),
        }
    }
}

/// A text document as the editor holds it: lines end in `\n`, with the original ending
/// remembered.
#[derive(Debug, Clone, PartialEq)]
pub struct TextFile {
    pub text: String,
    pub line_ending: LineEnding,
}

/// Reads a UTF-8 text file. A file that isn't UTF-8 is refused with a readable error, rather than
/// opened with replacement characters that the next autosave would write back.
pub fn read_text(path: &Path) -> io::Result<TextFile> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "not a UTF-8 text file"))?;
    let line_ending = LineEnding::detect(&text);
    Ok(TextFile {
        text: text.replace("\r\n", "\n"),
        line_ending,
    })
}

/// When the file on disk last changed and how long it is, to notice edits made outside this
/// window (another editor, a `git checkout`, or a second window of this app).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskStamp {
    modified: SystemTime,
    length: u64,
}

impl DiskStamp {
    /// The stamp of `path` now, or `None` if it can't be read (for example, it doesn't exist).
    pub fn of(path: &Path) -> Option<DiskStamp> {
        let meta = fs::metadata(path).ok()?;
        Some(DiskStamp {
            modified: meta.modified().ok()?,
            length: meta.len(),
        })
    }
}

/// Saves editor text to `path` with `line_ending`, unless the file changed on disk since this
/// window last read or wrote it (`expected`): then nothing is written and the error says so.
/// A file deleted outside the app is simply written again. Returns the new stamp.
pub fn save_text(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    expected: Option<DiskStamp>,
) -> io::Result<Option<DiskStamp>> {
    let current = DiskStamp::of(path);
    if current.is_some() && expected.is_some() && current != expected {
        return Err(io::Error::other("changed on disk outside this window"));
    }
    atomic_write(path, line_ending.apply(text).as_bytes())?;
    Ok(DiskStamp::of(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_line_endings() {
        assert_eq!(LineEnding::detect("a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("no breaks"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\nc\n"), LineEnding::Lf);
    }

    #[test]
    fn crlf_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.txt");
        fs::write(&path, "one\r\ntwo\r\n").unwrap();
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "one\ntwo\n");
        assert_eq!(file.line_ending, LineEnding::CrLf);
        assert_eq!(file.line_ending.apply(&file.text), "one\r\ntwo\r\n");
    }

    #[test]
    fn refuses_non_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin.txt");
        fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let error = read_text(&path).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "not a UTF-8 text file");
    }

    #[test]
    fn save_refuses_to_write_over_an_outside_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "original").unwrap();
        let stamp = DiskStamp::of(&path);
        let stamp = save_text(&path, "mine", LineEnding::Lf, stamp).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        // Someone else writes the file; a different length changes the stamp.
        fs::write(&path, "someone else's text").unwrap();
        let error = save_text(&path, "mine again", LineEnding::Lf, stamp).unwrap_err();
        assert_eq!(error.to_string(), "changed on disk outside this window");
        assert_eq!(fs::read_to_string(&path).unwrap(), "someone else's text");
    }

    #[test]
    fn save_writes_a_deleted_file_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "x").unwrap();
        let stamp = DiskStamp::of(&path);
        fs::remove_file(&path).unwrap();
        save_text(&path, "back", LineEnding::Lf, stamp).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "back");
    }

    #[test]
    fn missing_file_is_not_found() {
        let error = read_text(Path::new("/definitely/not/here.txt")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }
}
