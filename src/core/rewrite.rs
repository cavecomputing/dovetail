//! A rewrite's draft: a copy of a file's text that the window edits beside the original. It
//! autosaves to its own file in the data folder, so the original is only written when Ctrl+S
//! asks. The draft file lasting means a rewrite left open picks up again the next time its file
//! opens. Pausing a rewrite moves its draft aside, kept for the next Ctrl+R but not reopened with
//! the file. Scratch has no rewrite.

use std::fs;
use std::io::{self, ErrorKind};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use similar::{DiffTag, TextDiff};

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
    remove_file(&paused_path(data_dir, path))?;
    remove_file(&base_path(data_dir, path))
}

/// Where the rewrite of the file at `path` notes the original it started from: a fingerprint of
/// its text, then the file's path (so a later sweep can tell which file a draft belongs to).
fn base_path(data_dir: &Path, path: &Path) -> PathBuf {
    draft_path(data_dir, path).with_extension("base")
}

/// Notes `original`, the file's text as the rewrite of the file at `path` starts from it (when
/// it starts, and after each Ctrl+S).
pub fn write_base(data_dir: &Path, path: &Path, original: &str) -> io::Result<()> {
    let note = format!("{:016x}\n{}\n", fingerprint(original), path.display());
    atomic_write(&base_path(data_dir, path), note.as_bytes())
}

/// Whether the file at `path`, now holding `original`, changed since its rewrite started from
/// it: another program wrote it while the draft was closed or paused. A rewrite with no note
/// (from before notes were kept) counts as unchanged.
pub fn base_changed(data_dir: &Path, path: &Path, original: &str) -> bool {
    match fs::read_to_string(base_path(data_dir, path)) {
        Ok(note) => note.lines().next() != Some(&format!("{:016x}", fingerprint(original))),
        Err(_) => false,
    }
}

/// A fingerprint of `text` (64-bit FNV-1a), the same from one build of the app to the next,
/// which std's hasher doesn't promise.
fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What a draft changes from its original, letter by letter: the byte ranges of the original it takes
/// out, and of the draft it puts in.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Changes {
    pub removed: Vec<Range<usize>>,
    pub added: Vec<Range<usize>>,
}

/// The most text a block of changed lines may hold for its words to be compared, and a run of
/// changed words for its letters. Myers' search takes time in proportion to the text times the
/// number of differences, so a block this big that differs throughout would take seconds, and
/// is as good as lit whole. (A whole different text pasted in is one such block.)
const WORD_PASS_BYTES: usize = 64 * 1024;
const LETTER_PASS_BYTES: usize = 4 * 1024;

/// How long a whole comparison may take, all three passes together, as a safeguard behind the
/// size limits above; past it, the rest of the changed lines light whole. It runs off the UI
/// thread, so this is about the highlights not falling further behind the typing.
const DIFF_TIME: Duration = Duration::from_millis(500);

/// What `draft` changes from `original`, letter by letter. Lines are compared first, then only
/// the lines that differ word by word, then only the words that differ letter by letter: one
/// finer pass over a long text would take far longer for the same answer.
pub fn changes(original: &str, draft: &str) -> Changes {
    let deadline = Instant::now() + DIFF_TIME;
    let lines = TextDiff::configure()
        .deadline(deadline)
        .diff_lines(original, draft);
    let old_starts = starts(lines.iter_old_slices());
    let new_starts = starts(lines.iter_new_slices());
    let mut changes = Changes::default();
    for op in lines.ops() {
        if op.tag() == DiffTag::Equal {
            continue;
        }
        let old = old_starts[op.old_range().start]..old_starts[op.old_range().end];
        let new = new_starts[op.new_range().start]..new_starts[op.new_range().end];
        // Too big, or out of time: the changed lines light whole.
        if old.len() + new.len() > WORD_PASS_BYTES || Instant::now() >= deadline {
            light(Some((old, new)), original, draft, &mut changes);
            continue;
        }
        let words = TextDiff::configure()
            .deadline(deadline)
            .diff_words(&original[old.clone()], &draft[new.clone()]);
        let old_words = starts(words.iter_old_slices());
        let new_words = starts(words.iter_new_slices());
        for op in words.ops() {
            if op.tag() == DiffTag::Equal {
                continue;
            }
            let removed = old.start + old_words[op.old_range().start]
                ..old.start + old_words[op.old_range().end];
            let added = new.start + new_words[op.new_range().start]
                ..new.start + new_words[op.new_range().end];
            if removed.len() + added.len() > LETTER_PASS_BYTES || Instant::now() >= deadline {
                light(Some((removed, added)), original, draft, &mut changes);
                continue;
            }
            letters(original, removed, draft, added, deadline, &mut changes);
        }
    }
    changes
}

impl Changes {
    /// Moves the lit ranges of the draft along with an edit to it, until the next comparison
    /// comes back: `at` is where the edit began, `removed` how many bytes it took out and
    /// `inserted` how many it put in. Ranges before the edit stay, ranges after it shift, and the
    /// edit itself, with any range it touched, is lit as one run (what was put in is new text).
    pub fn shift(&mut self, at: usize, removed: usize, inserted: usize) {
        let removed_end = at + removed;
        let mut edit = at..at + inserted;
        let mut shifted = Vec::with_capacity(self.added.len() + 1);
        for range in &self.added {
            if range.end < at || (range.end == at && inserted == 0) {
                shifted.push(range.clone());
            } else if range.start > removed_end {
                shifted.push(range.start + inserted - removed..range.end + inserted - removed);
            } else {
                // Touches the edit (typing right next to a lit run counts): joined with it, the
                // part past the edit moved along.
                edit.start = edit.start.min(range.start);
                if range.end > removed_end {
                    edit.end = edit.end.max(range.end + inserted - removed);
                }
            }
        }
        if !edit.is_empty() {
            shifted.push(edit);
        }
        shifted.sort_by_key(|range| range.start);
        self.added = shifted;
    }
}

/// The byte offset each of `pieces` starts at, and the end of the last.
fn starts<'a>(pieces: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let mut starts = vec![0];
    for piece in pieces {
        starts.push(starts[starts.len() - 1] + piece.len());
    }
    starts
}

/// How few letters left alike between changed ones are lit with them, so a word swapped for
/// another doesn't light up around the odd letter they share.
const SHARED_LETTERS: usize = 3;

/// Adds the words `removed` from `original` and `added` in their place in `draft` to `changes`,
/// letter by letter: the letters both keep at their ends stay unlit, and so do runs of at least
/// `SHARED_LETTERS` between changed letters.
fn letters(
    original: &str,
    removed: Range<usize>,
    draft: &str,
    added: Range<usize>,
    deadline: Instant,
    changes: &mut Changes,
) {
    let diff = TextDiff::configure()
        .deadline(deadline)
        .diff_chars(&original[removed.clone()], &draft[added.clone()]);
    let old_letters = starts(diff.iter_old_slices());
    let new_letters = starts(diff.iter_new_slices());
    let ops = diff.ops();
    let mut lit: Option<(Range<usize>, Range<usize>)> = None;
    for (index, op) in ops.iter().enumerate() {
        let old = removed.start + old_letters[op.old_range().start]
            ..removed.start + old_letters[op.old_range().end];
        let new = added.start + new_letters[op.new_range().start]
            ..added.start + new_letters[op.new_range().end];
        let between = index > 0 && index + 1 < ops.len();
        if op.tag() == DiffTag::Equal && !(between && op.old_range().len() < SHARED_LETTERS) {
            light(lit.take(), original, draft, changes);
            continue;
        }
        lit = match lit {
            Some((lit_old, lit_new)) => Some((lit_old.start..old.end, lit_new.start..new.end)),
            None => Some((old, new)),
        };
    }
    light(lit, original, draft, changes);
}

/// Adds a run of letters taken out of `original` and put into `draft` to `changes`.
fn light(
    run: Option<(Range<usize>, Range<usize>)>,
    original: &str,
    draft: &str,
    changes: &mut Changes,
) {
    let Some((removed, added)) = run else {
        return;
    };
    if !removed.is_empty() {
        extend(&mut changes.removed, removed, original);
    }
    if !added.is_empty() {
        extend(&mut changes.added, added, draft);
    }
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
    fn changes_are_found_letter_by_letter() {
        let original = "Always write in a friendly tone.\nKeep it short.\n";
        let draft = "Write in a warm tone.\nKeep it short.\nThanks!\n";
        let found = changes(original, draft);
        let words = |text: &str, ranges: &[Range<usize>]| -> Vec<String> {
            ranges
                .iter()
                .map(|range| text[range.clone()].to_string())
                .collect()
        };
        // A word swapped for another lights whole, not around the `r` they share.
        assert_eq!(words(original, &found.removed), ["Always w", "friendly"]);
        assert_eq!(words(draft, &found.added), ["W", "warm", "Thanks!\n"]);
        // Changed words with only a space between them are one run (`each` keeps the `e` of
        // `every`).
        let draft = "make each line short";
        let found = changes("keep every entry short", draft);
        assert_eq!(words(draft, &found.added), ["make", "ach line"]);
        assert_eq!(changes(draft, draft), Changes::default());
        // Inside a word, only the letters that changed.
        let original = "narrating for {{user}}. Then a colour";
        let draft = "narrating for {{usr}}. Then a color";
        let found = changes(original, draft);
        assert_eq!(words(original, &found.removed), ["e", "u"]);
        assert!(found.added.is_empty());
        let draft = "narrating for {testing}. Then a colour";
        let found = changes(original, draft);
        assert_eq!(words(original, &found.removed), ["{user}"]);
        assert_eq!(words(draft, &found.added), ["testing"]);
    }

    #[test]
    fn a_few_letters_changed_in_a_long_text_light_only_themselves() {
        // Long enough that a word by word diff of it all runs out of time, which lit everything
        // between the two changes.
        let original: String = (0..50_000)
            .map(|line| format!("line {line} says the same few words again\n"))
            .collect();
        let draft = (original.replacen("line 2500 says", "line 2500 sayz", 1)).replacen(
            "40000 says the same",
            "40000 says the sane",
            1,
        );
        let found = changes(&original, &draft);
        let words = |text: &str, ranges: &[Range<usize>]| -> Vec<String> {
            ranges
                .iter()
                .map(|range| text[range.clone()].to_string())
                .collect()
        };
        assert_eq!(words(&original, &found.removed), ["s", "m"]);
        assert_eq!(words(&draft, &found.added), ["z", "n"]);
    }

    #[test]
    fn lit_ranges_move_along_with_an_edit_until_the_next_comparison() {
        let mut lit = Changes {
            removed: vec![0..3, 7..9],
            added: vec![2..4, 10..14, 20..22],
        };
        // Typed in unchanged text between two runs: they stay and shift, the typing is lit.
        lit.shift(6, 0, 3);
        assert_eq!(lit.added, [2..4, 6..9, 13..17, 23..25]);
        // Deleted across the end of a run: what's left of it stays lit, the rest moves up.
        lit.shift(15, 4, 0);
        assert_eq!(lit.added, [2..4, 6..9, 13..15, 19..21]);
        // Replaced over two runs: one run from the first to the end of what was put in.
        lit.shift(8, 6, 2);
        assert_eq!(lit.added, [2..4, 6..11, 15..17]);
        // A deletion that touches nothing lit only shifts.
        lit.shift(0, 1, 0);
        assert_eq!(lit.added, [1..3, 5..10, 14..16]);
        assert_eq!(
            lit.removed,
            vec![0..3, 7..9],
            "the original is left as it was"
        );
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

    #[test]
    fn a_rewrite_notices_its_file_changed_since_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.md");
        assert!(
            !base_changed(dir.path(), &notes, "# notes\n"),
            "no note yet"
        );
        write_base(dir.path(), &notes, "# notes\n").unwrap();
        assert!(!base_changed(dir.path(), &notes, "# notes\n"));
        assert!(base_changed(
            dir.path(),
            &notes,
            "# notes, edited elsewhere\n"
        ));
        let note = fs::read_to_string(base_path(dir.path(), &notes)).unwrap();
        assert!(note.ends_with(&format!("{}\n", notes.display())), "{note}");
        remove_draft(dir.path(), &notes).unwrap();
        assert!(!base_path(dir.path(), &notes).exists());
    }
}
