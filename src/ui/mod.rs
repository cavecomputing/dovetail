//! Everything iced: the windows, their widgets and styling. Logic lives in `core/`.

pub mod app;
pub mod assistant_actions;
pub mod assistant_view;
pub mod editor;
pub mod file_actions;
pub mod find_bar;
pub mod highlighter;
pub mod key_guard;
pub mod palette;
pub mod picker;
pub mod scroll_bar;
pub mod settings_panel;
pub mod status_bar;
pub mod switcher;
pub mod text_editor;
pub mod theme;
// Only Wayland needs a drop listener of our own; elsewhere winit hears drops.
#[cfg(all(unix, not(target_os = "macos")))]
pub mod wayland_drop;
pub mod window_controls;
