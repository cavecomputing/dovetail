//! Entry point: reads the command line and decides what this window opens.
//!
//! - no arguments: the scratch space
//! - a path: that text file, created if missing
//! - `--print-brand`: prints the names from `brand.rs`, and the version, for the install scripts,
//!   then exits

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
    let doc = match argument {
        Some(flag) if flag == Path::new("--print-brand") => {
            print_brand();
            return Ok(());
        }
        Some(path) => Doc::File {
            // An absolute path, so a window opened from here still finds it.
            path: std::path::absolute(&path).unwrap_or(path),
            line_ending: Default::default(),
            bom: false,
        },
        None => Doc::Scratch,
    };
    // Explorer starts a program in the opened file's folder, and Windows won't let a folder be
    // renamed or deleted while it is a program's current folder, so move to the home folder (the
    // path above is already absolute).
    #[cfg(windows)]
    if let Some(base) = directories::BaseDirs::new() {
        let _ = std::env::set_current_dir(base.home_dir());
    }
    ui::app::run(doc)
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
