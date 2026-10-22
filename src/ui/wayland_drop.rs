//! Files dropped on the window under Wayland. winit, under iced, hears a drop only on X11, macOS
//! and Windows (`window::Event::FileDropped`, see `app::shortcut`). Under Wayland the clipboard's
//! data device hears it instead: Hyprland tells only an app's first data device about a drag, and
//! that is the clipboard's, so our patched copy of the clipboard crate (vendor/smithay-clipboard)
//! hands each drop's `text/uri-list` over. Under X11 it never calls.

use iced::Task;
use iced::futures::StreamExt;
use iced::futures::channel::mpsc::unbounded;
use iced::futures::stream;

use crate::core::uri_list::files_in_uri_list;
use crate::ui::app::Message;

/// Opens each file dropped on the window from now on (`Message::OpenPath`).
pub fn listen() -> Task<Message> {
    let (sender, lists) = unbounded();
    smithay_clipboard::on_drop(move |list| {
        let _ = sender.unbounded_send(list);
    });
    let files = lists.flat_map(|list| stream::iter(files_in_uri_list(&list)));
    Task::run(files, Message::OpenPath)
}
