//! The 32px top strip with the app-drawn window controls, and the edges that resize an
//! undecorated window. Under a tiling window manager both are left out. On macOS the window keeps
//! the system's see-through title bar: its buttons sit over the strip and it resizes the window,
//! so the strip is always there, empty, and no edges are drawn.

use iced::widget::{button, container, mouse_area, row, space, stack, text};
use iced::window::Direction;
use iced::{Element, Fill, Length, mouse};

use crate::core::settings::WindowControls;
use crate::ui::app::Message;
use crate::ui::theme::{GLYPH_CLOSE, GLYPH_MAXIMIZE, GLYPH_MINIMIZE, icon_button};

const STRIP_HEIGHT: f32 = 32.0;
const EDGE: f32 = 6.0;
/// macOS frames the window itself (see `app::window_settings`).
const NATIVE_FRAME: bool = cfg!(target_os = "macos");

/// Whether to draw the window controls, after the Auto/Show/Hide setting.
pub fn visible(setting: WindowControls) -> bool {
    // macOS's buttons need the strip under them, or they would sit over the text.
    if NATIVE_FRAME {
        return true;
    }
    match setting {
        WindowControls::Auto => !tiling_window_manager(),
        WindowControls::Show => true,
        WindowControls::Hide => false,
    }
}

/// Tiling window managers tile and move windows themselves, so the controls only get in the way.
fn tiling_window_manager() -> bool {
    let sockets = [
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "I3SOCK",
        "NIRI_SOCKET",
    ];
    if sockets.iter().any(|name| std::env::var_os(name).is_some()) {
        return true;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    ["hyprland", "sway", "niri", "river", "i3"]
        .iter()
        .any(|name| desktop.contains(name))
}

/// The top strip: a drag area with minimize, maximize and close at the right (on macOS, the
/// system's own buttons at the left).
pub fn strip<'a>() -> Element<'a, Message> {
    let control = |glyph, message| {
        button(text(glyph).size(13).center())
            .width(28)
            .height(24)
            .padding(0)
            .style(icon_button)
            .on_press(message)
    };
    let controls = match NATIVE_FRAME {
        true => row![],
        false => row![
            control(GLYPH_MINIMIZE, Message::Minimize),
            control(GLYPH_MAXIMIZE, Message::ToggleMaximize),
            control(GLYPH_CLOSE, Message::CloseRequested),
        ]
        .spacing(6),
    };
    mouse_area(
        container(row![space::horizontal(), controls])
            .height(STRIP_HEIGHT)
            .padding([0, 4])
            .center_y(STRIP_HEIGHT),
    )
    .on_press(Message::Drag)
    .on_double_click(Message::ToggleMaximize)
    .into()
}

/// Thin grab areas on the four edges and in the corners, laid over `page`. While the controls are
/// hidden the areas are left empty, keeping the layout's shape.
pub fn with_resize_edges(page: Element<'_, Message>, shown: bool) -> Element<'_, Message> {
    if NATIVE_FRAME {
        return page;
    }
    let edge = |width: Length, height: Length, cursor, direction| -> Element<'static, Message> {
        let area = space().width(width).height(height);
        match shown {
            true => mouse_area(area)
                .interaction(cursor)
                .on_press(Message::Resize(direction))
                .into(),
            false => area.into(),
        }
    };
    let side = |direction| {
        edge(
            Length::Fixed(EDGE),
            Fill,
            mouse::Interaction::ResizingHorizontally,
            direction,
        )
    };
    let corner = |cursor, direction| {
        edge(
            Length::Fixed(EDGE * 2.0),
            Length::Fixed(EDGE * 2.0),
            cursor,
            direction,
        )
    };
    // The top corners reach only as far down as the edge, leaving the strip's buttons their clicks.
    let top_corner = |cursor, direction| {
        edge(
            Length::Fixed(EDGE * 2.0),
            Length::Fixed(EDGE),
            cursor,
            direction,
        )
    };
    stack![
        page,
        container(side(Direction::West)).align_left(Fill),
        container(side(Direction::East)).align_right(Fill),
        container(edge(
            Fill,
            Length::Fixed(EDGE),
            mouse::Interaction::ResizingVertically,
            Direction::South
        ))
        .align_bottom(Fill),
        container(corner(
            mouse::Interaction::ResizingDiagonallyUp,
            Direction::SouthWest
        ))
        .align_left(Fill)
        .align_bottom(Fill),
        container(corner(
            mouse::Interaction::ResizingDiagonallyDown,
            Direction::SouthEast
        ))
        .align_right(Fill)
        .align_bottom(Fill),
        edge(
            Fill,
            Length::Fixed(EDGE),
            mouse::Interaction::ResizingVertically,
            Direction::North
        ),
        top_corner(
            mouse::Interaction::ResizingDiagonallyDown,
            Direction::NorthWest
        ),
        container(top_corner(
            mouse::Interaction::ResizingDiagonallyUp,
            Direction::NorthEast
        ))
        .align_right(Fill),
    ]
    .into()
}

/// macOS's Quit (⌘Q, or the app menu) would end the process at once, skipping the wait for the
/// latest save. This points it at closing the window instead, which waits; each window is its own
/// process, so that still quits. Needs the titled window from `app::window_settings`, which
/// answers `performClose:`.
#[cfg(target_os = "macos")]
pub fn quit_closes_window() {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::NSApplication;

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let app_menu = NSApplication::sharedApplication(main_thread)
        .mainMenu()
        .and_then(|menu_bar| menu_bar.itemAtIndex(0))
        .and_then(|item| item.submenu());
    let Some(app_menu) = app_menu else {
        return;
    };
    for index in 0..app_menu.numberOfItems() {
        if let Some(item) = app_menu.itemAtIndex(index)
            && item.action() == Some(sel!(terminate:))
        {
            // SAFETY: performClose: is NSWindow's own action, taking a sender as terminate: does.
            unsafe { item.setAction(Some(sel!(performClose:))) };
        }
    }
}
