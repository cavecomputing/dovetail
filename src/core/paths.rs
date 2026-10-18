//! Where config and scratch state live on disk, and where file dialogs start.

use std::path::{Path, PathBuf};

use directories::{BaseDirs, UserDirs};

use crate::brand::CONFIG_DIR_NAME;

/// The folders the app reads and writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    /// `~/.config/<app>`: holds `settings.toml`.
    pub config_dir: PathBuf,
    /// `~/.local/share/<app>`: holds the scratch space.
    pub data_dir: PathBuf,
    /// `~/Documents`: where the file dialogs start when there is no open file to start beside.
    pub documents_dir: PathBuf,
}

impl Paths {
    /// The real folders for this user. If the environment variable named by
    /// `home_override_var()` is set, everything goes under that folder instead, which keeps test
    /// runs and screenshots away from the real config.
    pub fn detect() -> Paths {
        if let Some(root) = std::env::var_os(home_override_var()) {
            return Paths::under(Path::new(&root));
        }
        let base = BaseDirs::new().expect("the home directory is always known on Linux");
        let documents_dir = UserDirs::new()
            .and_then(|dirs| dirs.document_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| base.home_dir().join("Documents"));
        Paths {
            config_dir: base.config_dir().join(CONFIG_DIR_NAME),
            data_dir: base.data_dir().join(CONFIG_DIR_NAME),
            documents_dir,
        }
    }

    /// All three folders inside `root`, for tests and the `<APP>_HOME` override.
    pub fn under(root: &Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            documents_dir: root.join("documents"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }
}

/// A path for showing to people, with the home folder written as `~`.
pub fn display_path(path: &Path) -> String {
    match BaseDirs::new().and_then(|base| {
        path.strip_prefix(base.home_dir())
            .ok()
            .map(Path::to_path_buf)
    }) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Name of the environment variable that moves every folder, derived from the brand so a rename
/// changes it too: `CONFIG_DIR_NAME` in capitals, dashes as underscores, plus `_HOME`.
pub fn home_override_var() -> String {
    format!("{}_HOME", CONFIG_DIR_NAME.to_uppercase().replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_puts_everything_inside_root() {
        let paths = Paths::under(Path::new("/tmp/x"));
        assert_eq!(paths.config_dir, Path::new("/tmp/x/config"));
        assert_eq!(paths.data_dir, Path::new("/tmp/x/data"));
        assert_eq!(paths.documents_dir, Path::new("/tmp/x/documents"));
        assert_eq!(
            paths.settings_file(),
            Path::new("/tmp/x/config/settings.toml")
        );
    }

    #[test]
    fn override_var_comes_from_brand() {
        let var = home_override_var();
        assert!(var.ends_with("_HOME"));
        assert!(!var.contains('-'));
        assert_eq!(var, var.to_uppercase());
    }

    #[test]
    fn detect_uses_brand_folder_names() {
        // Only meaningful when the override is not set in the test environment.
        if std::env::var_os(home_override_var()).is_some() {
            return;
        }
        let paths = Paths::detect();
        assert!(paths.config_dir.ends_with(CONFIG_DIR_NAME));
        assert!(paths.data_dir.ends_with(CONFIG_DIR_NAME));
    }

    #[test]
    fn display_path_shortens_home() {
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(display_path(&home.join("notes.txt")), "~/notes.txt");
        assert_eq!(display_path(Path::new("/etc/hosts")), "/etc/hosts");
    }
}
