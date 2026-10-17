//! Versions as git commits. Each version is one commit tagged `v<number>` holding `draft.txt` and
//! the manifest. The writer never sees any of this; the repo is just the project's hidden `.git` folder.

use std::path::Path;

use git2::Repository;

use crate::brand::{APP_NAME, BIN_NAME};

/// Makes an empty repo in `dir` with the app as its committer, so commits work on a machine with
/// no git config at all.
pub fn init(dir: &Path) -> anyhow::Result<()> {
    let repo = Repository::init(dir)?;
    let mut config = repo.config()?;
    config.set_str("user.name", APP_NAME)?;
    config.set_str("user.email", &format!("noreply@{BIN_NAME}"))?;
    Ok(())
}

/// Commits exactly `files` (paths inside `dir`) as version `number` and tags it `v<number>`.
/// Anything else in the folder is left out, and files from earlier commits that aren't listed
/// (such as the manifest's old name after a rename) are dropped.
pub fn commit_version(dir: &Path, files: &[&Path], number: u32, note: &str) -> anyhow::Result<()> {
    let repo = Repository::open(dir)?;
    let mut index = repo.index()?;
    index.clear()?;
    for file in files {
        index.add_path(file)?;
    }
    index.write()?;
    let tree = repo.find_tree(index.write_tree()?)?;

    let message = match note.trim() {
        "" => format!("v{number}"),
        note => format!("v{number}: {note}"),
    };
    let signature = repo.signature()?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(_) => None, // the very first commit has no parent
    };
    let parents: Vec<_> = parent.iter().collect();
    let commit = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &parents,
    )?;
    repo.tag_lightweight(
        &format!("v{number}"),
        &repo.find_object(commit, None)?,
        false,
    )?;
    Ok(())
}

/// The text of `file` as committed in version `number`.
pub fn committed_text(dir: &Path, number: u32, file: &str) -> anyhow::Result<String> {
    let repo = Repository::open(dir)?;
    let blob = repo
        .revparse_single(&format!("v{number}:{file}"))?
        .peel_to_blob()?;
    Ok(String::from_utf8_lossy(blob.content()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_tags_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one").unwrap();
        commit_version(dir.path(), &[Path::new("a.txt")], 1, "").unwrap();
        std::fs::write(&file, "two").unwrap();
        commit_version(dir.path(), &[Path::new("a.txt")], 2, "tighter").unwrap();

        assert_eq!(committed_text(dir.path(), 1, "a.txt").unwrap(), "one");
        assert_eq!(committed_text(dir.path(), 2, "a.txt").unwrap(), "two");
        assert!(committed_text(dir.path(), 3, "a.txt").is_err());

        let repo = Repository::open(dir.path()).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap(), "v2: tighter");
        assert_eq!(head.parent(0).unwrap().message().unwrap(), "v1");
        assert_eq!(head.author().name().unwrap(), APP_NAME);
    }

    #[test]
    fn only_listed_files_are_committed() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap();
        std::fs::write(dir.path().join("old.json"), "{}").unwrap();
        std::fs::write(dir.path().join("stray.txt"), "x").unwrap();
        commit_version(dir.path(), &[Path::new("old.json")], 1, "").unwrap();
        std::fs::rename(dir.path().join("old.json"), dir.path().join("new.json")).unwrap();
        commit_version(dir.path(), &[Path::new("new.json")], 2, "").unwrap();

        assert!(committed_text(dir.path(), 2, "new.json").is_ok());
        assert!(committed_text(dir.path(), 2, "old.json").is_err());
        assert!(committed_text(dir.path(), 1, "stray.txt").is_err());
    }
}
