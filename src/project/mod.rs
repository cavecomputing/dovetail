//! Project mode logic. Nothing in here imports iced, so all of it can be tested without a window.
//!
//! A project is a folder holding a JSON manifest (`<name>.<PROJECT_EXT>`), `draft.txt` (the text
//! being worked on) and a hidden git repo with one commit per version.

pub mod history;
pub mod manifest;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::brand::PROJECT_EXT;
use crate::core::save::atomic_write;
use manifest::{Manifest, Version};

/// The file holding the current draft, inside the project folder.
pub const DRAFT_FILE: &str = "draft.txt";

/// How many words of the text name a new project's folder.
const NAME_WORDS: usize = 3;
/// Longest folder name, in characters, so long first words still make a usable path.
const NAME_MAX_CHARS: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// The project folder; its last part is the project's name.
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    #[allow(dead_code)] // Only tests so far; the window title reads the manifest's file name.
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(manifest_file_name(&self.manifest.name))
    }

    /// Where the draft autosaves. Write it with `core::save::atomic_write`.
    pub fn draft_path(&self) -> PathBuf {
        self.dir.join(DRAFT_FILE)
    }

    /// The draft as last saved. Missing means empty.
    #[allow(dead_code)] // Only tests; a window reads the draft through `core::opened`.
    pub fn draft(&self) -> String {
        fs::read_to_string(self.draft_path()).unwrap_or_default()
    }

    /// The newest version's number (1 right after the project is created).
    pub fn latest_version(&self) -> u32 {
        self.manifest.latest_version()
    }

    /// The text of version `number` (1 to `latest_version()`) as it was committed.
    pub fn version_text(&self, number: u32) -> anyhow::Result<String> {
        history::committed_text(&self.dir, number, DRAFT_FILE)
    }

    /// Saves `draft` and the manifest and commits them as the next version. Returns its number.
    pub fn commit(&mut self, draft: &str, note: &str) -> anyhow::Result<u32> {
        let number = self.latest_version() + 1;
        let mut manifest = self.manifest.clone();
        manifest.versions.push(Version {
            number,
            note: note.trim().to_string(),
            committed_at: now(),
            paint: Vec::new(),
        });
        atomic_write(&self.draft_path(), draft.as_bytes())?;
        manifest.save(&self.manifest_path())?;
        let manifest_file = manifest_file_name(&manifest.name);
        let files = [Path::new(DRAFT_FILE), Path::new(&manifest_file)];
        if let Err(error) = history::commit_version(&self.dir, &files, number, note) {
            // Put the old manifest back so it never lists a version git doesn't have.
            let _ = self.manifest.save(&self.manifest_path());
            return Err(error);
        }
        self.manifest = manifest;
        Ok(number)
    }

    /// Renames the project: moves its folder and renames its manifest. The name is cleaned up for
    /// use as a folder name first. Fails if another project already has that name.
    #[allow(dead_code)] // Until the UI can rename a project; remove then.
    pub fn rename(&mut self, new_name: &str) -> anyhow::Result<()> {
        let name = folder_name(new_name, usize::MAX);
        if name == self.manifest.name {
            return Ok(());
        }
        let parent = self.dir.parent().unwrap_or(Path::new(".")).to_path_buf();
        let new_dir = parent.join(&name);
        if new_dir.exists() {
            anyhow::bail!("a project called {name} already exists");
        }
        let old_manifest = manifest_file_name(&self.manifest.name);
        fs::rename(&self.dir, &new_dir)?;
        self.dir = new_dir;
        fs::rename(
            self.dir.join(old_manifest),
            self.dir.join(manifest_file_name(&name)),
        )?;
        self.manifest.name = name;
        self.manifest.save(&self.manifest_path())
    }
}

/// Makes a new project under `projects_dir` from `text`: a folder named after the text's first
/// few words, the manifest, `draft.txt`, and git with `text` committed as v1.
pub fn create_project(projects_dir: &Path, text: &str) -> anyhow::Result<Project> {
    let base = folder_name(text, NAME_WORDS);
    let mut name = base.clone();
    let mut n = 2;
    while projects_dir.join(&name).exists() {
        name = format!("{base} {n}");
        n += 1;
    }
    let dir = projects_dir.join(&name);
    fs::create_dir_all(&dir)?;
    history::init(&dir)?;
    let mut project = Project {
        dir,
        manifest: Manifest::new(&name),
    };
    project.commit(text, "")?;
    Ok(project)
}

/// Opens the project whose manifest is at `manifest_path`.
pub fn open_project(manifest_path: &Path) -> anyhow::Result<Project> {
    let manifest = Manifest::load(manifest_path)?;
    let dir = match manifest_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Ok(Project { dir, manifest })
}

/// True if `path` is a project manifest rather than a text file.
pub fn is_project(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == PROJECT_EXT)
}

/// One row of the Ctrl+P switcher. `core::recent` keeps .txt files and manifests in one list;
/// this tells them apart and gives each a name to show.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentItem {
    pub path: PathBuf,
    /// The file name for a text file, the project name for a project.
    pub label: String,
    pub is_project: bool,
}

/// The recent files and projects, newest first, leaving out any that no longer exist (a project
/// that was renamed, a file that was deleted).
pub fn recent_items(data_dir: &Path) -> Vec<RecentItem> {
    crate::core::recent::recent_files(data_dir)
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| {
            let is_project = is_project(&path);
            let label = match is_project {
                true => path.file_stem(),
                false => path.file_name(),
            };
            RecentItem {
                label: label.unwrap_or_default().to_string_lossy().into_owned(),
                is_project,
                path,
            }
        })
        .collect()
}

fn manifest_file_name(name: &str) -> String {
    format!("{name}.{PROJECT_EXT}")
}

/// A folder name from the first `words` words of `text`: letters, digits, spaces, `-`, `_` and
/// `'` only, so it is safe on any filesystem. "Untitled" if nothing is left.
fn folder_name(text: &str, words: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c.is_alphanumeric() || "-_'".contains(c) {
            true => c,
            false => ' ',
        })
        .collect();
    let name: String = cleaned
        .split_whitespace()
        .take(words)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(NAME_MAX_CHARS)
        .collect();
    match name.trim() {
        "" => "Untitled".to_string(),
        name => name.to_string(),
    }
}

/// Seconds since 1970.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_come_from_the_first_words() {
        assert_eq!(folder_name("Nyx is a quiet cat.", 3), "Nyx is a");
        assert_eq!(folder_name("  Nyx!  ", 3), "Nyx");
        assert_eq!(folder_name("../../etc/passwd", 3), "etc passwd");
        assert_eq!(folder_name(".hidden", 3), "hidden");
        assert_eq!(folder_name("Ünïcödé 夜 café", 3), "Ünïcödé 夜 café");
        assert_eq!(folder_name("   ", 3), "Untitled");
        assert_eq!(folder_name("🙂 🙂", 3), "Untitled");
        assert_eq!(
            folder_name(&"a".repeat(100), 3).chars().count(),
            NAME_MAX_CHARS
        );
    }

    #[test]
    fn create_makes_folder_manifest_draft_and_v1() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "Nyx is a quiet cat").unwrap();
        assert_eq!(project.name(), "Nyx is a");
        assert_eq!(project.dir, root.path().join("Nyx is a"));
        assert!(
            project
                .manifest_path()
                .ends_with(format!("Nyx is a.{PROJECT_EXT}"))
        );
        assert!(project.dir.join(".git").is_dir());
        assert_eq!(project.draft(), "Nyx is a quiet cat");
        assert_eq!(project.latest_version(), 1);
        assert_eq!(project.version_text(1).unwrap(), "Nyx is a quiet cat");
        let committed_manifest =
            history::committed_text(&project.dir, 1, &manifest_file_name("Nyx is a")).unwrap();
        assert!(committed_manifest.contains("\"number\": 1"));
    }

    #[test]
    fn name_collisions_get_a_number() {
        let root = tempfile::tempdir().unwrap();
        let first = create_project(root.path(), "Nyx").unwrap();
        let second = create_project(root.path(), "Nyx").unwrap();
        let third = create_project(root.path(), "Nyx.").unwrap();
        assert_eq!(first.name(), "Nyx");
        assert_eq!(second.name(), "Nyx 2");
        assert_eq!(third.name(), "Nyx 3");
        assert_eq!(second.version_text(1).unwrap(), "Nyx");
    }

    #[test]
    fn commits_three_times_and_reads_every_version() {
        let root = tempfile::tempdir().unwrap();
        let mut project = create_project(root.path(), "one").unwrap();
        assert_eq!(project.commit("two", "shorter").unwrap(), 2);
        assert_eq!(project.commit("three", "").unwrap(), 3);
        assert_eq!(project.commit("four", " warmer ").unwrap(), 4);

        for (number, text) in [(1, "one"), (2, "two"), (3, "three"), (4, "four")] {
            assert_eq!(project.version_text(number).unwrap(), text);
        }
        assert!(project.version_text(5).is_err());
        assert_eq!(project.draft(), "four");
        let notes: Vec<_> = project
            .manifest
            .versions
            .iter()
            .map(|v| v.note.as_str())
            .collect();
        assert_eq!(notes, ["", "shorter", "", "warmer"]);

        let reopened = open_project(&project.manifest_path()).unwrap();
        assert_eq!(reopened, project);
    }

    #[test]
    fn draft_saved_between_commits_is_read_back() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "start").unwrap();
        atomic_write(&project.draft_path(), b"work in progress").unwrap();
        let reopened = open_project(&project.manifest_path()).unwrap();
        assert_eq!(reopened.draft(), "work in progress");
        assert_eq!(reopened.version_text(1).unwrap(), "start");
    }

    #[test]
    fn rename_moves_folder_and_manifest_and_keeps_history() {
        let root = tempfile::tempdir().unwrap();
        let mut project = create_project(root.path(), "Nyx").unwrap();
        project.commit("v2 text", "").unwrap();
        let old_dir = project.dir.clone();

        project.rename("Nyx / Night 夜").unwrap();
        assert_eq!(project.name(), "Nyx Night 夜");
        assert!(!old_dir.exists());
        assert!(project.manifest_path().is_file());
        assert_eq!(project.version_text(1).unwrap(), "Nyx");

        // The next commit holds the manifest under its new name only.
        project.commit("v3 text", "after rename").unwrap();
        let reopened = open_project(&project.manifest_path()).unwrap();
        assert_eq!(reopened.name(), "Nyx Night 夜");
        assert_eq!(reopened.latest_version(), 3);
        assert_eq!(reopened.version_text(3).unwrap(), "v3 text");
        assert!(history::committed_text(&reopened.dir, 3, &manifest_file_name("Nyx")).is_err());
    }

    #[test]
    fn rename_refuses_a_taken_name_and_ignores_the_same_name() {
        let root = tempfile::tempdir().unwrap();
        create_project(root.path(), "Taken").unwrap();
        let mut project = create_project(root.path(), "Mine").unwrap();
        assert!(project.rename("Taken").is_err());
        assert_eq!(project.name(), "Mine");
        project.rename("Mine").unwrap();
        assert!(project.manifest_path().is_file());
    }

    #[test]
    fn recent_items_mix_files_and_projects() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let project = create_project(&root.path().join("projects"), "Nyx").unwrap();
        let notes = root.path().join("notes.txt");
        fs::write(&notes, "hi").unwrap();
        crate::core::recent::remember(&data, &root.path().join("gone.txt")).unwrap();
        crate::core::recent::remember(&data, &notes).unwrap();
        crate::core::recent::remember(&data, &project.manifest_path()).unwrap();

        let items = recent_items(&data);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Nyx");
        assert!(items[0].is_project);
        assert_eq!(items[1].label, "notes.txt");
        assert!(!items[1].is_project);
    }
}
