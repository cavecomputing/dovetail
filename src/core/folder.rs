//! Folders: what's in one, for the folder pane's tree and the Ctrl+O picker; every file under
//! one, for the pane's filter; and how the picker reads the path typed into it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use crate::core::recent::match_score;

/// Folders never listed or searched: version control's own, and JavaScript's installed packages.
const SKIPPED: [&str; 2] = [".git", "node_modules"];
/// The most files the pane's filter searches; in a bigger folder it searches this many.
pub const MAX_FILES: usize = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Entry {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
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
        .filter(|entry| !SKIPPED.contains(&entry.name().as_str()))
        .collect();
    found.sort_by_key(|entry| (!entry.is_dir, entry.name().to_lowercase()));
    found
}

/// One row of the pane's tree: an entry, and how deep it is (0 for the folder's own).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
}

/// The tree under `root`: its entries, each folder in `expanded` followed by its own.
pub fn tree(root: &Path, expanded: &BTreeSet<PathBuf>) -> Vec<Row> {
    let mut rows = Vec::new();
    add_rows(root, 0, expanded, &mut rows);
    rows
}

fn add_rows(dir: &Path, depth: usize, expanded: &BTreeSet<PathBuf>, rows: &mut Vec<Row>) {
    for entry in entries(dir) {
        let inside = (entry.is_dir && expanded.contains(&entry.path)).then(|| entry.path.clone());
        rows.push(Row { entry, depth });
        if let Some(inside) = inside {
            add_rows(&inside, depth + 1, expanded, rows);
        }
    }
}

/// Every file under `root`, relative to it, up to `MAX_FILES`, and whether it stopped there. A
/// link to a folder isn't followed, so a loop of links ends.
pub fn files_under(root: &Path) -> (Vec<PathBuf>, bool) {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(dir) = folders.pop() {
        for entry in entries(&dir) {
            if entry.is_dir {
                let link = fs::symlink_metadata(&entry.path).is_ok_and(|meta| meta.is_symlink());
                if !link {
                    folders.push(entry.path);
                }
            } else if files.len() == MAX_FILES {
                return (files, true);
            } else {
                let relative = entry.path.strip_prefix(root).unwrap_or(&entry.path);
                files.push(relative.to_path_buf());
            }
        }
    }
    (files, false)
}

/// The files whose path matches `query` (see `match_score`), best first, and the shorter path
/// first among equals.
pub fn matching<'a>(files: &'a [PathBuf], query: &str) -> Vec<&'a PathBuf> {
    let mut scored: Vec<(i32, &PathBuf)> = files
        .iter()
        .filter_map(|path| Some((match_score(query, &path.to_string_lossy())?, path)))
        .collect();
    scored.sort_by_key(|(score, path)| (-score, path.as_os_str().len()));
    scored.into_iter().map(|(_, path)| path).collect()
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

/// The picker's rows for `name` typed in `folder`: with nothing typed, the folder itself first
/// (to open it in the pane), then its entries; with a name, the entries matching it, best first.
/// Hidden entries (a leading `.`) show only once the name starts with one.
pub fn picker_rows(folder: &Path, name: &str) -> Vec<Entry> {
    let shown = entries(folder)
        .into_iter()
        .filter(|entry| name.starts_with('.') || !entry.name().starts_with('.'));
    if name.is_empty() {
        let itself = Entry {
            path: folder.to_path_buf(),
            is_dir: true,
        };
        return std::iter::once(itself).chain(shown).collect();
    }
    let mut scored: Vec<(i32, Entry)> = shown
        .filter_map(|entry| Some((match_score(name, &entry.name())?, entry)))
        .collect();
    // A stable sort keeps folders first among equal scores.
    scored.sort_by_key(|(score, _)| -score);
    scored.into_iter().map(|(_, entry)| entry).collect()
}

/// `typed` with the name after its last separator completed to `entry`'s, and a separator after
/// a folder's, so the picker then lists that folder.
pub fn complete(typed: &str, entry: &Entry) -> String {
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
    fn entries_put_folders_first_and_skip_git() {
        let root = folder_with("entries", &["b.txt", "A.md", "src/", ".git/x", "zeta/"]);
        assert_eq!(names(&entries(&root)), ["src", "zeta", "A.md", "b.txt"]);
        assert!(entries(&root.join("missing")).is_empty());
    }

    #[test]
    fn the_tree_opens_expanded_folders_in_place() {
        let root = folder_with("tree", &["a/one.txt", "a/b/two.txt", "c.txt"]);
        let mut expanded = BTreeSet::new();
        let shown = |expanded: &BTreeSet<PathBuf>| -> Vec<(String, usize)> {
            let rows = tree(&root, expanded);
            rows.iter()
                .map(|row| (row.entry.name(), row.depth))
                .collect()
        };
        assert_eq!(shown(&expanded), [("a".into(), 0), ("c.txt".into(), 0)]);
        expanded.insert(root.join("a"));
        expanded.insert(root.join("a/b"));
        assert_eq!(
            shown(&expanded),
            [
                ("a".into(), 0),
                ("b".into(), 1),
                ("two.txt".into(), 2),
                ("one.txt".into(), 1),
                ("c.txt".into(), 0)
            ]
        );
    }

    #[test]
    fn the_filter_searches_every_file_by_its_path() {
        let root = folder_with(
            "filter",
            &["deploy.sh", "scripts/backup.sh", "notes.txt", ".git/HEAD"],
        );
        let (files, capped) = files_under(&root);
        assert!(!capped);
        assert_eq!(files.len(), 3, "the .git folder is left out");
        let found: Vec<String> = matching(&files, "sh")
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(found, ["deploy.sh", "scripts/backup.sh"]);
        assert!(matching(&files, "zzz").is_empty());
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
    fn picker_rows_offer_the_folder_then_matches() {
        let root = folder_with("picker", &["deploy.sh", "docs/", ".env", "notes.txt"]);
        let rows = picker_rows(&root, "");
        assert_eq!(rows[0].path, root, "the folder itself comes first");
        assert_eq!(names(&rows[1..]), ["docs", "deploy.sh", "notes.txt"]);
        assert_eq!(names(&picker_rows(&root, "dep")), ["deploy.sh"]);
        assert_eq!(names(&picker_rows(&root, ".e")), [".env"]);
    }
}
