//! Entry point: reads the command line and decides what this window opens.
//!
//! - no arguments: the scratch space
//! - a path: that text file, created if missing
//! - `--print-brand`: prints the names from `brand.rs` for `scripts/install.sh`, then exits

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

/// Shell variable lines, e.g. `APP_NAME='...'`, for the install script to `eval`.
fn print_brand() {
    let names = [
        ("APP_NAME", brand::APP_NAME),
        ("BIN_NAME", brand::BIN_NAME),
        ("APP_ID", brand::APP_ID),
    ];
    for (name, value) in names {
        println!("{name}='{}'", value.replace('\'', r"'\''"));
    }
}
