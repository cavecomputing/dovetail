//! Files dropped on the window under Wayland. winit, under iced, hears a drop only on X11, macOS
//! and Windows (`window::Event::FileDropped`, see `app::shortcut`), so under Wayland this listens
//! on the window's own connection, on a thread of its own, for the `text/uri-list` a file manager
//! offers. Under X11 there is no Wayland display, and nothing here starts.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::Task;
use iced::futures::Stream;
use iced::futures::channel::mpsc::{UnboundedSender, unbounded};
use iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};

use crate::core::uri_list::files_in_uri_list;
use crate::ui::app::Message;

const URI_LIST: &str = "text/uri-list";

/// Opens each file dropped on `window` from now on (`Message::OpenPath`), when it is a Wayland
/// window.
pub fn listen(window: iced::window::Id) -> Task<Message> {
    iced::window::run(window, handles).then(|handles| match handles {
        Some(handles) => Task::run(dropped_files(handles), Message::OpenPath),
        None => Task::none(),
    })
}

/// The window's Wayland display and surface, as addresses, so they can go to the drop thread.
struct Handles {
    display: usize,
    surface: usize,
}

/// The window's handles, or `None` when it isn't a Wayland window.
fn handles(window: &dyn iced::window::Window) -> Option<Handles> {
    let display = match window.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Wayland(handle) => handle.display.as_ptr() as usize,
        _ => return None,
    };
    let surface = match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Wayland(handle) => handle.surface.as_ptr() as usize,
        _ => return None,
    };
    Some(Handles { display, surface })
}

/// Each file dropped on the window from now on.
fn dropped_files(handles: Handles) -> impl Stream<Item = PathBuf> {
    let (sender, files) = unbounded();
    // Without the thread, drops just don't open anything.
    let _ = std::thread::Builder::new()
        .name("drops".to_string())
        .spawn(move || hear_drops(handles, sender));
    files
}

/// Listens for drops until the connection fails, which is how the app ending reaches this thread.
fn hear_drops(handles: Handles, sender: UnboundedSender<PathBuf>) -> Option<()> {
    // Safety: winit's display and surface live as long as the window, which is as long as the app.
    let backend = unsafe { Backend::from_foreign_display(handles.display as *mut _) };
    let surface = unsafe { ObjectId::from_ptr(WlSurface::interface(), handles.surface as *mut _) };
    let connection = Connection::from_backend(backend);
    let (globals, mut queue) = registry_queue_init::<Drops>(&connection).ok()?;
    let queue_handle = queue.handle();
    let manager: WlDataDeviceManager = globals.bind(&queue_handle, 1..=3, ()).ok()?;
    let seat: WlSeat = globals.bind(&queue_handle, 1..=1, ()).ok()?;
    manager.get_data_device(&seat, &queue_handle, ());
    let mut drops = Drops {
        surface: surface.ok()?,
        sender,
        connection,
        offer: None,
    };
    while queue.blocking_dispatch(&mut drops).is_ok() {}
    // The dispatch fails when the window closes: winit disconnects the display this thread
    // shares while the main thread is still on its way out. Dropping what's above would then
    // destroy our proxies and queue on that gone display, which crashed the exit (a segfault in
    // libwayland's object table). So nothing is released here; the process ending frees it all.
    std::mem::forget((queue, drops, globals, manager, seat));
    Some(())
}

struct Drops {
    surface: ObjectId,
    sender: UnboundedSender<PathBuf>,
    connection: Connection,
    /// The drag over the window, while it holds files.
    offer: Option<WlDataOffer>,
}

/// What a drag says it holds, as its offer's events tell it.
#[derive(Default)]
struct Offered {
    files: AtomicBool,
    /// Whether the drag will copy, rather than move (which would delete the dragged file).
    copy: AtomicBool,
}

impl Drops {
    /// The text the drag holds as `text/uri-list`.
    fn uri_list(&self, offer: &WlDataOffer) -> Option<String> {
        let (mut reader, writer) = std::io::pipe().ok()?;
        offer.receive(URI_LIST.to_string(), std::os::fd::AsFd::as_fd(&writer));
        // Only the other app may hold the pipe open, so the read ends when it has written all.
        drop(writer);
        self.connection.flush().ok()?;
        let mut list = String::new();
        reader.read_to_string(&mut list).ok()?;
        Some(list)
    }
}

impl Dispatch<WlDataDevice, ()> for Drops {
    fn event(
        drops: &mut Self,
        _device: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Enter {
                serial,
                surface,
                id: Some(offer),
                ..
            } => {
                let files = offer
                    .data::<Offered>()
                    .is_some_and(|o| o.files.load(Ordering::Relaxed));
                if surface.id() != drops.surface || !files {
                    offer.destroy();
                    return;
                }
                offer.accept(serial, Some(URI_LIST.to_string()));
                if offer.version() >= 3 {
                    offer.set_actions(DndAction::Copy, DndAction::Copy);
                }
                drops.offer = Some(offer);
            }
            wl_data_device::Event::Leave => {
                if let Some(offer) = drops.offer.take() {
                    offer.destroy();
                }
            }
            wl_data_device::Event::Drop => {
                let Some(offer) = drops.offer.take() else {
                    return;
                };
                let list = drops.uri_list(&offer).unwrap_or_default();
                for file in files_in_uri_list(&list) {
                    let _ = drops.sender.unbounded_send(file);
                }
                // `finish` is a protocol error, which would end the window's connection too,
                // unless the drag settled on a copy.
                let copy = offer
                    .data::<Offered>()
                    .is_some_and(|o| o.copy.load(Ordering::Relaxed));
                if offer.version() >= 3 && copy {
                    offer.finish();
                }
                offer.destroy();
            }
            // The clipboard's offers: iced reads the clipboard on a connection of its own.
            wl_data_device::Event::Selection { id: Some(offer) } => offer.destroy(),
            _ => {}
        }
    }

    event_created_child!(Drops, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Offered::default()),
    ]);
}

impl Dispatch<WlDataOffer, Offered> for Drops {
    fn event(
        _: &mut Self,
        _offer: &WlDataOffer,
        event: wl_data_offer::Event,
        offered: &Offered,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } if mime_type == URI_LIST => {
                offered.files.store(true, Ordering::Relaxed);
            }
            wl_data_offer::Event::Action { dnd_action } => {
                let copy = dnd_action
                    .into_result()
                    .is_ok_and(|action| action == DndAction::Copy);
                offered.copy.store(copy, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

// Nothing to hear from these.
impl Dispatch<WlRegistry, GlobalListContents> for Drops {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Drops {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for Drops {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
