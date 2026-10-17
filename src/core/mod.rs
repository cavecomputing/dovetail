//! Plain-editor logic. Nothing in here imports iced, so all of it can be tested without a window.

pub mod counts;
pub mod lock;
pub mod opened;
#[allow(dead_code)] // Until project mode paints; remove then.
pub mod paint;
pub mod paths;
pub mod recent;
pub mod save;
pub mod scratch;
pub mod settings;
pub mod text_file;
pub mod text_model;
