//! Every name the app shows or writes to disk comes from here, so renaming the app means editing
//! this one file (plus `name` in Cargo.toml, which sets the binary's file name).

/// The app's display name, used in window titles, the `.desktop` entry and folder names.
pub const APP_NAME: &str = "Dovetail";

/// The binary and command name. Must match `[package] name` in Cargo.toml.
pub const BIN_NAME: &str = "dovetail";

/// Reverse-DNS application id: the Wayland app id and the `.desktop` file name.
pub const APP_ID: &str = "io.cavecomputing.Dovetail";

/// Folder name under `~/.config/` and `~/.local/share/`.
pub const CONFIG_DIR_NAME: &str = "dovetail";

/// File extension of a project manifest, without the dot. Projects live on the rewrite branch,
/// which shares the data folder, so their manifests are left out of the recent lists here.
pub const PROJECT_EXT: &str = "dovetail";
