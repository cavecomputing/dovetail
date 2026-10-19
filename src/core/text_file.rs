//! Reading plain text documents (`.txt`, Markdown's `.md`, and data files such as `.json`).

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
    /// A lone `\r`, as classic Mac OS wrote.
    Cr,
}

impl LineEnding {
    /// The most common ending in `text`; LF when there are no line breaks.
    pub fn detect(text: &str) -> LineEnding {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        let cr = text.matches('\r').count() - crlf;
        if cr > lf && cr > crlf {
            LineEnding::Cr
        } else if crlf > lf {
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
            LineEnding::Cr => text.replace('\n', "\r"),
        }
    }
}

/// A text document as the editor holds it: lines end in `\n`, with the original ending
/// remembered.
#[derive(Debug, Clone, PartialEq)]
pub struct TextFile {
    pub text: String,
    pub line_ending: LineEnding,
    /// Whether the file starts with a byte order mark, which is kept out of `text`.
    pub bom: bool,
}

/// The byte order mark some Windows programs (Notepad, PowerShell) start a UTF-8 file with.
const BOM: char = '\u{feff}';

/// Reads a UTF-8 text file. A file that isn't UTF-8 is refused with a readable error, rather than
/// opened with replacement characters that the next autosave would write back.
pub fn read_text(path: &Path) -> io::Result<TextFile> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "not a UTF-8 text file"))?;
    // Left in the text, the mark would be an invisible character that typing at the very start
    // moves into the first word.
    let (text, bom) = match text.strip_prefix(BOM) {
        Some(rest) => (rest, true),
        None => (text.as_str(), false),
    };
    Ok(TextFile {
        text: newlines_only(text),
        line_ending: LineEnding::detect(text),
        bom,
    })
}

/// `text` with every line ending (`\r\n`, or a lone `\r` from an old Mac file or a terminal) as
/// `\n`, the only ending the editor's line and column arithmetic understands.
pub fn newlines_only(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
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

/// Saves editor text to `path` with `line_ending` (and a byte order mark first if `bom`), unless
/// the file changed on disk since this window last read or wrote it (`expected`): then nothing is
/// written and the error says so. A file deleted outside the app is simply written again.
/// Returns the new stamp.
pub fn save_text(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    expected: Option<DiskStamp>,
) -> io::Result<Option<DiskStamp>> {
    if changed_on_disk(path, expected) {
        return Err(io::Error::other("changed on disk"));
    }
    let mut disk_text = line_ending.apply(text);
    if bom {
        disk_text.insert(0, BOM);
    }
    atomic_write(path, disk_text.as_bytes())?;
    Ok(DiskStamp::of(path))
}

/// True when the file at `path` is not the one this window last read or wrote (`expected`).
/// A missing file, or nothing seen yet, doesn't count.
pub fn changed_on_disk(path: &Path, expected: Option<DiskStamp>) -> bool {
    let current = DiskStamp::of(path);
    current.is_some() && expected.is_some() && current != expected
}

/// A file name for saving `text` that has none yet: its first words, or "untitled".
pub fn suggested_file_name(text: &str) -> String {
    let first_line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let words: Vec<&str> = first_line.split_whitespace().take(6).collect();
    // Only letters, digits and a little punctuation, so the name works on any file system.
    let name: String = words
        .join(" ")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | ','))
        .take(60)
        .collect();
    match name.trim_matches(|c: char| c == ' ' || c == '.') {
        "" => "untitled.txt".to_string(),
        name => format!("{name}.txt"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_name_comes_from_the_first_words() {
        assert_eq!(
            suggested_file_name("\n  Meeting notes: Sam\nmore"),
            "Meeting notes Sam.txt"
        );
        assert_eq!(
            suggested_file_name("<system_instructions>"),
            "system_instructions.txt"
        );
        assert_eq!(
            suggested_file_name("one two three four five six seven"),
            "one two three four five six.txt"
        );
        assert_eq!(suggested_file_name("  \n../"), "untitled.txt");
        assert_eq!(suggested_file_name(""), "untitled.txt");
    }

    #[test]
    fn detects_line_endings() {
        assert_eq!(LineEnding::detect("a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("no breaks"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\nc\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\rb\r"), LineEnding::Cr);
        assert_eq!(LineEnding::detect("a\rb\r\nc\r\n"), LineEnding::CrLf);
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
    fn cr_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mac.txt");
        fs::write(&path, "mac\rold\rstyle\r").unwrap();
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "mac\nold\nstyle\n");
        assert_eq!(file.line_ending, LineEnding::Cr);
        assert_eq!(file.line_ending.apply(&file.text), "mac\rold\rstyle\r");
    }

    #[test]
    fn a_byte_order_mark_stays_out_of_the_text_and_is_saved_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notepad.txt");
        fs::write(&path, "\u{feff}first\r\nsecond\r\n").unwrap();
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "first\nsecond\n");
        assert!(file.bom);
        assert_eq!(file.line_ending, LineEnding::CrLf);
        let stamp = DiskStamp::of(&path);
        save_text(&path, "X", file.line_ending, file.bom, stamp).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfX");
        // A mark anywhere else is the writer's own text.
        fs::write(&path, "a\u{feff}b").unwrap();
        let file = read_text(&path).unwrap();
        assert_eq!((file.text.as_str(), file.bom), ("a\u{feff}b", false));
    }

    #[test]
    fn lone_carriage_returns_become_newlines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mac.txt");
        fs::write(&path, "a\rb\n\rc\r\nd").unwrap();
        assert_eq!(read_text(&path).unwrap().text, "a\nb\n\nc\nd");
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
        let stamp = save_text(&path, "mine", LineEnding::Lf, false, stamp).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        // Someone else writes the file; a different length changes the stamp.
        fs::write(&path, "someone else's text").unwrap();
        assert!(changed_on_disk(&path, stamp));
        let error = save_text(&path, "mine again", LineEnding::Lf, false, stamp).unwrap_err();
        assert_eq!(error.to_string(), "changed on disk");
        assert_eq!(fs::read_to_string(&path).unwrap(), "someone else's text");
    }

    #[test]
    fn save_writes_a_deleted_file_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "x").unwrap();
        let stamp = DiskStamp::of(&path);
        fs::remove_file(&path).unwrap();
        save_text(&path, "back", LineEnding::Lf, false, stamp).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "back");
    }

    #[test]
    fn missing_file_is_not_found() {
        let error = read_text(Path::new("/definitely/not/here.txt")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }
}
