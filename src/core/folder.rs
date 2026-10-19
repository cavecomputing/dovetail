//! Folders for the Ctrl+O picker: what's in one, and how the picker reads the path typed into it.

use std::fs;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use crate::core::paths::display_path;
use crate::core::recent::match_score;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Entry {
    pub fn name(&self) -> String {
        if self.is_up() {
            return "..".to_string();
        }
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether this is the picker's `..` row, the folder above the listed one.
    pub fn is_up(&self) -> bool {
        self.path.ends_with("..")
    }
}

/// What's in `dir`, folders first, then by name ignoring case. An unreadable folder is empty.
pub fn entries(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Entry> = read
        .flatten()
        .map(|item| {
            let path = item.path();
            Entry {
                is_dir: path.is_dir(),
                path,
            }
        })
        .collect();
    found.sort_by_key(|entry| (!entry.is_dir, entry.name().to_lowercase()));
    found
}

/// Whether `c` separates folders in a typed path: `/`, and on Windows `\` too.
fn is_separator(c: char) -> bool {
    c == '/' || c == MAIN_SEPARATOR
}

/// The Ctrl+O picker's box read as a path: the folder it lists (everything up to the last
/// separator, with `~` and any relative path taken from `home`) and the name typed after it.
pub fn split_typed(typed: &str, home: &Path) -> (PathBuf, String) {
    let (folder, name) = match typed.rfind(is_separator) {
        Some(at) => (&typed[..=at], &typed[at + 1..]),
        None => ("", typed),
    };
    let folder = match folder.strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches(is_separator)),
        None => home.join(folder),
    };
    // Rebuilt from its parts, the folder has no separator at the end.
    (folder.components().collect(), name.to_string())
}

/// The picker's rows for `name` typed in `folder`: with nothing typed, `..` (unless the folder
/// is the top one) and then its entries; with a name, the entries matching it, best first.
/// Hidden entries (a leading `.`) show only once the name starts with one.
pub fn picker_rows(folder: &Path, name: &str) -> Vec<Entry> {
    let shown = entries(folder)
        .into_iter()
        .filter(|entry| name.starts_with('.') || !entry.name().starts_with('.'));
    if name.is_empty() {
        let up = folder.parent().map(|_| Entry {
            path: folder.join(".."),
            is_dir: true,
        });
        return up.into_iter().chain(shown).collect();
    }
    let mut scored: Vec<(i32, Entry)> = shown
        .filter_map(|entry| Some((match_score(name, &entry.name())?, entry)))
        .collect();
    // A stable sort keeps folders first among equal scores.
    scored.sort_by_key(|(score, _)| -score);
    scored.into_iter().map(|(_, entry)| entry).collect()
}

/// `typed` with the name after its last separator completed to `entry`'s, and a separator after
/// a folder's, so the picker then lists that folder. `..` gives the folder above, written out.
pub fn complete(typed: &str, entry: &Entry) -> String {
    if entry.is_up() {
        let above = entry.path.parent().and_then(Path::parent);
        return typed_folder(above.map(display_path).unwrap_or_else(|| typed.to_string()));
    }
    let folder = match typed.rfind(is_separator) {
        Some(at) => &typed[..=at],
        None => "",
    };
    let mut completed = format!("{folder}{}", entry.name());
    if entry.is_dir {
        completed.push(MAIN_SEPARATOR);
    }
    completed
}

/// What the picker's box starts with: `folder` as the status bar writes it (`~` for the home
/// folder), ending in a separator.
pub fn typed_folder(shown: String) -> String {
    match shown.ends_with(is_separator) {
        true => shown,
        false => format!("{shown}{MAIN_SEPARATOR}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder in the temp dir holding `files` (a trailing `/` makes a folder).
    fn folder_with(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("folder-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            match file.ends_with('/') {
                true => fs::create_dir_all(&path).unwrap(),
                false => {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(&path, "x").unwrap();
                }
            }
        }
        root
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(Entry::name).collect()
    }

    #[test]
    fn entries_put_folders_first() {
        let root = folder_with("entries", &["b.txt", "A.md", "src/", "zeta/"]);
        assert_eq!(names(&entries(&root)), ["src", "zeta", "A.md", "b.txt"]);
        assert!(entries(&root.join("missing")).is_empty());
    }

    #[test]
    fn the_picker_reads_the_typed_path() {
        let home = Path::new("/home/me");
        let sep = MAIN_SEPARATOR;
        let split = |typed: &str| split_typed(typed, home);
        assert_eq!(split("~/code/ho"), (home.join("code"), "ho".to_string()));
        assert_eq!(split("~/"), (home.to_path_buf(), String::new()));
        assert_eq!(split("notes"), (home.to_path_buf(), "notes".to_string()));
        let entry = |path: &str, is_dir| Entry {
            path: PathBuf::from(path),
            is_dir,
        };
        assert_eq!(
            complete("~/code/ho", &entry("/x/homelab", true)),
            format!("~/code/homelab{sep}")
        );
        assert_eq!(
            complete("~/code/ho", &entry("/x/hosts", false)),
            "~/code/hosts"
        );
        assert_eq!(typed_folder("~/code".into()), format!("~/code{sep}"));
        assert_eq!(typed_folder("~/".into()), "~/");
    }

    #[test]
    fn picker_rows_are_the_folder_or_the_matches() {
        let root = folder_with("picker", &["deploy.sh", "docs/", ".env", "notes.txt"]);
        let rows = picker_rows(&root, "");
        assert_eq!(names(&rows), ["..", "docs", "deploy.sh", "notes.txt"]);
        // `..` goes to the folder above, and the top folder has none.
        let above = typed_folder(display_path(root.parent().unwrap()));
        assert_eq!(complete("anything/", &rows[0]), above);
        let top = root.ancestors().last().unwrap();
        assert!(!picker_rows(top, "").iter().any(Entry::is_up));
        assert_eq!(names(&picker_rows(&root, "dep")), ["deploy.sh"]);
        assert_eq!(names(&picker_rows(&root, ".e")), [".env"]);
    }
}
