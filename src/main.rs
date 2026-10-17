//! Entry point: reads the command line and decides what this window opens.
//!
//! - no arguments: the scratch space
//! - a project manifest (`.<PROJECT_EXT>`): that project
//! - any other path: that text file, created if missing
//! - `--print-brand`: prints the names from `brand.rs` for `scripts/install.sh`, then exits

mod brand;
mod core;
mod project;
mod ui;

use std::path::{Path, PathBuf};

use crate::core::opened::Doc;

fn main() -> iced::Result {
    let argument = std::env::args_os().nth(1).map(PathBuf::from);
    let doc = match argument {
        Some(flag) if flag == Path::new("--print-brand") => {
            print_brand();
            return Ok(());
        }
        Some(path) => {
            // An absolute path, so a window opened from here still finds it.
            let path = std::path::absolute(&path).unwrap_or(path);
            if path
                .extension()
                .is_some_and(|ext| ext == brand::PROJECT_EXT)
            {
                Doc::Project(path)
            } else {
                Doc::File {
                    path,
                    line_ending: Default::default(),
                }
            }
        }
        None => Doc::Scratch,
    };
    ui::app::run(doc)
}

/// Shell variable lines, e.g. `APP_NAME='...'`, for the install script to `eval`.
fn print_brand() {
    let names = [
        ("APP_NAME", brand::APP_NAME),
        ("BIN_NAME", brand::BIN_NAME),
        ("APP_ID", brand::APP_ID),
        ("PROJECT_EXT", brand::PROJECT_EXT),
        ("PROJECT_MIME", brand::PROJECT_MIME),
    ];
    for (name, value) in names {
        println!("{name}='{}'", value.replace('\'', r"'\''"));
    }
}
