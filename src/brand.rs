//! Every name the app shows or writes to disk comes from here, so renaming the app means editing
//! this one file (plus `name` in Cargo.toml, which sets the binary's file name).

/// The app's display name, used in window titles, the `.desktop` entry and folder names.
pub const APP_NAME: &str = "Word Designer";

/// The binary and command name. Must match `[package] name` in Cargo.toml.
pub const BIN_NAME: &str = "word-designer";

/// Reverse-DNS application id: the Wayland app id, the `.desktop` file name and the MIME prefix.
pub const APP_ID: &str = "io.cavecomputing.WordDesigner";

/// File extension of a project manifest, without the dot.
pub const PROJECT_EXT: &str = "wdp";

/// MIME type registered for project manifests.
pub const PROJECT_MIME: &str = "application/x-word-designer-project";

/// Folder name under `~/.config/` and `~/.local/share/`.
pub const CONFIG_DIR_NAME: &str = "word-designer";

/// Folder under `~/Documents/` where new projects are created.
pub const PROJECTS_DIR_NAME: &str = "Word Designer";
