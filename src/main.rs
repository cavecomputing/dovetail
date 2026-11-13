//! Entry point: reads the command line and decides what this window opens.
//!
//! - no arguments: the scratch space
//! - a path: that text file (one that doesn't exist yet is created by its first save)
//! - a folder: the scratch space, with the Ctrl+O picker open on that folder
//! - `--print-brand`: prints the names from `brand.rs`, and the version, for the install scripts,
//!   then exits
//! - `--version`: prints the name and version, then exits
//! - `--help` or `-h`: prints what the command line takes, then exits

// A release build on Windows is a GUI program, so no console window opens beside it. Its output
// still reaches a pipe or a file (`--print-brand` in a script); debug builds keep the console, for
// error messages.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod brand;
mod core;
mod ui;

use std::path::{Path, PathBuf};

use crate::core::opened::Doc;

fn main() -> iced::Result {
    let argument = std::env::args_os()
        .nth(1)
        // Finder may start a macOS app with a -psn_... process number, which isn't a file.
        .filter(|arg| !(cfg!(target_os = "macos") && arg.to_string_lossy().starts_with("-psn_")))
        .map(PathBuf::from);
    let mut folder = None;
    let doc = match argument {
        Some(flag) if flag == Path::new("--print-brand") => {
            print_brand();
            return Ok(());
        }
        Some(flag) if flag == Path::new("--version") => {
            println!("{} {}", brand::BIN_NAME, env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        // Before, these opened a new file named `--help`.
        Some(flag) if flag == Path::new("--help") || flag == Path::new("-h") => {
            let bin = brand::BIN_NAME;
            println!("Usage: {bin} [FILE]");
            println!();
            println!("  {bin}              open the scratch space");
            println!("  {bin} FILE         open a text file (a new name is a new file)");
            println!("  {bin} FOLDER       choose a file in that folder");
            println!("  {bin} HOST:PATH    open a file on an SSH host, named as in the SSH config");
            println!("  {bin} --version    print the version");
            println!("  {bin} --help       print this");
            return Ok(());
        }
        // `alias:path` is a file on an SSH host, unless this computer has a file by that name
        // (`./a:b` names one that isn't there yet).
        Some(path) if !path.exists() && matches!(Doc::recent(&path), Doc::Remote { .. }) => {
            Doc::recent(&path)
        }
        // A folder: scratch, with the Ctrl+O picker open on the folder.
        Some(path) if path.is_dir() => {
            folder = Some(std::path::absolute(&path).unwrap_or(path));
            Doc::Scratch
        }
        // An absolute path, so a window opened from here still finds it.
        Some(path) => Doc::file(std::path::absolute(&path).unwrap_or(path)),
        None => Doc::Scratch,
    };
    // Explorer starts a program in the opened file's folder, and Windows won't let a folder be
    // renamed or deleted while it is a program's current folder, so move to the home folder (the
    // path above is already absolute).
    #[cfg(windows)]
    if let Some(base) = directories::BaseDirs::new() {
        let _ = std::env::set_current_dir(base.home_dir());
    }
    core::shutdown::write_unsaved_at_end();
    let ran = ui::app::run(doc, folder);
    // The window can end without closing, as when the display server goes away under it.
    core::shutdown::write_unsaved();
    ran
}

/// Shell variable lines, e.g. `APP_NAME='...'`, for the install scripts: the bash ones `eval`
/// them, the PowerShell one reads them line by line.
fn print_brand() {
    let names = [
        ("APP_NAME", brand::APP_NAME),
        ("BIN_NAME", brand::BIN_NAME),
        ("APP_ID", brand::APP_ID),
        ("CONFIG_DIR_NAME", brand::CONFIG_DIR_NAME),
        ("VERSION", env!("CARGO_PKG_VERSION")),
    ];
    for (name, value) in names {
        println!("{name}='{}'", value.replace('\'', r"'\''"));
    }
}
