//! The 32px top strip with the app-drawn window controls, and the edges that resize an
//! undecorated window. Under a tiling window manager both are left out.

use iced::widget::{button, container, mouse_area, row, space, stack, text};
use iced::window::{self, Direction};
use iced::{Element, Fill, Length, mouse};

use crate::core::settings::WindowControls;
use crate::ui::app::Message;
use crate::ui::theme::{GLYPH_CLOSE, GLYPH_MAXIMIZE, GLYPH_MINIMIZE, icon_button};

const STRIP_HEIGHT: f32 = 32.0;
const EDGE: f32 = 6.0;

/// Whether to draw the window controls, after the Auto/Show/Hide setting.
pub fn visible(setting: WindowControls) -> bool {
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

/// The top strip: a drag area with minimize, maximize and close at the right.
pub fn strip<'a>(window: window::Id) -> Element<'a, Message> {
    let control = |glyph, message| {
        button(text(glyph).size(13).center())
            .width(28)
            .height(24)
            .padding(0)
            .style(icon_button)
            .on_press(message)
    };
    let controls = row![
        control(GLYPH_MINIMIZE, Message::Minimize(window)),
        control(GLYPH_MAXIMIZE, Message::ToggleMaximize(window)),
        control(GLYPH_CLOSE, Message::CloseRequested(window)),
    ]
    .spacing(6);
    mouse_area(
        container(row![space::horizontal(), controls])
            .height(STRIP_HEIGHT)
            .padding([0, 4])
            .center_y(STRIP_HEIGHT),
    )
    .on_press(Message::Drag(window))
    .on_double_click(Message::ToggleMaximize(window))
    .into()
}

/// Thin grab areas on the left, right and bottom edges (and the bottom corners) laid over `page`.
pub fn with_resize_edges<'a>(
    page: Element<'a, Message>,
    window: window::Id,
) -> Element<'a, Message> {
    let edge = |width: Length, height: Length, cursor, direction| {
        mouse_area(space().width(width).height(height))
            .interaction(cursor)
            .on_press(Message::Resize(window, direction))
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
    ]
    .into()
}
