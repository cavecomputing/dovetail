//! The editor's scroll bar: the same rail and handle as Settings' scrollable, beside a text the
//! editor scrolls itself (`Content::scroll_rows`). Dragging the handle, or pressing the rail,
//! scrolls the editor by whole rows.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Shell, mouse, renderer};
use iced::widget::scrollable::{Status, default};
use iced::{Element, Event, Length, Rectangle, Size, Theme};

use crate::ui::text_editor::ScrollRows;

/// As wide as Settings' bar (iced's default).
const WIDTH: f32 = 10.0;
/// The shortest the handle gets, so a long text still leaves something to grab.
const MIN_HANDLE: f32 = 24.0;

/// A scroll bar for a view at `rows`; `on_scroll` gets how many rows to scroll by.
pub struct ScrollBar<Message> {
    rows: ScrollRows,
    on_scroll: fn(i32) -> Message,
}

pub fn scroll_bar<Message>(rows: ScrollRows, on_scroll: fn(i32) -> Message) -> ScrollBar<Message> {
    ScrollBar { rows, on_scroll }
}

/// Where the handle was grabbed, down from its top, while it is dragged.
#[derive(Default)]
struct Grab(Option<f32>);

impl<Message> ScrollBar<Message> {
    /// The handle in `rail`, or none when the whole text fits.
    fn handle(&self, rail: Rectangle) -> Option<Rectangle> {
        let ScrollRows { top, shown, total } = self.rows;
        if total <= shown {
            return None;
        }
        let height = (rail.height * shown / total)
            .max(MIN_HANDLE)
            .min(rail.height);
        let travel = rail.height - height;
        let y = rail.y + travel * (top / (total - shown)).clamp(0.0, 1.0);
        Some(Rectangle { y, height, ..rail })
    }

    /// The rows to scroll by so the handle's top goes to `y`.
    fn scroll_to(&self, rail: Rectangle, handle: Rectangle, y: f32) -> i32 {
        let ScrollRows { top, shown, total } = self.rows;
        let travel = (rail.height - handle.height).max(1.0);
        let fraction = ((y - rail.y) / travel).clamp(0.0, 1.0);
        (fraction * (total - shown) - top).round() as i32
    }
}

impl<Message, Renderer: renderer::Renderer> Widget<Message, Theme, Renderer>
    for ScrollBar<Message>
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<Grab>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(Grab::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(WIDTH), Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.width(WIDTH).height(Length::Fill).max())
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn iced::advanced::Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let rail = layout.bounds();
        let Some(handle) = self.handle(rail) else {
            return;
        };
        let grab = tree.state.downcast_mut::<Grab>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(point) = cursor.position_over(rail) else {
                    return;
                };
                // On the handle it holds where it was grabbed; elsewhere on the rail the handle
                // jumps there, centred under the pointer, and can be dragged on from there.
                let offset = match handle.contains(point) {
                    true => point.y - handle.y,
                    false => handle.height / 2.0,
                };
                grab.0 = Some(offset);
                let rows = self.scroll_to(rail, handle, point.y - offset);
                if rows != 0 {
                    shell.publish((self.on_scroll)(rows));
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(offset) = grab.0 {
                    let rows = self.scroll_to(rail, handle, position.y - offset);
                    if rows != 0 {
                        shell.publish((self.on_scroll)(rows));
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if grab.0.is_some() => {
                grab.0 = None;
                shell.request_redraw();
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        match self.handle(layout.bounds()).is_some() && cursor.is_over(layout.bounds()) {
            true => mouse::Interaction::Idle,
            false => mouse::Interaction::None,
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let rail = layout.bounds();
        let Some(handle) = self.handle(rail) else {
            return;
        };
        let dragged = tree.state.downcast_ref::<Grab>().0.is_some();
        let hovered = cursor.is_over(rail);
        let status = match (dragged, hovered) {
            (true, _) => Status::Dragged {
                is_horizontal_scrollbar_dragged: false,
                is_vertical_scrollbar_dragged: true,
                is_horizontal_scrollbar_disabled: true,
                is_vertical_scrollbar_disabled: false,
            },
            (false, true) => Status::Hovered {
                is_horizontal_scrollbar_hovered: false,
                is_vertical_scrollbar_hovered: true,
                is_horizontal_scrollbar_disabled: true,
                is_vertical_scrollbar_disabled: false,
            },
            (false, false) => Status::Active {
                is_horizontal_scrollbar_disabled: true,
                is_vertical_scrollbar_disabled: false,
            },
        };
        let look = default(theme, status).vertical_rail;
        if let Some(background) = look.background {
            let quad = renderer::Quad {
                bounds: rail,
                border: look.border,
                ..renderer::Quad::default()
            };
            renderer.fill_quad(quad, background);
        }
        let quad = renderer::Quad {
            bounds: handle,
            border: look.scroller.border,
            ..renderer::Quad::default()
        };
        renderer.fill_quad(quad, look.scroller.background);
    }
}

impl<'a, Message: 'a, Renderer: renderer::Renderer + 'a> From<ScrollBar<Message>>
    for Element<'a, Message, Theme, Renderer>
{
    fn from(bar: ScrollBar<Message>) -> Self {
        Element::new(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(top: f32, shown: f32, total: f32) -> ScrollBar<i32> {
        scroll_bar(ScrollRows { top, shown, total }, |rows| rows)
    }

    const RAIL: Rectangle = Rectangle {
        x: 0.0,
        y: 100.0,
        width: WIDTH,
        height: 400.0,
    };

    #[test]
    fn no_handle_when_the_whole_text_fits() {
        assert_eq!(bar(0.0, 20.0, 20.0).handle(RAIL), None);
    }

    #[test]
    fn the_handle_shows_how_much_is_in_view_and_where() {
        let top = bar(0.0, 20.0, 100.0).handle(RAIL).unwrap();
        assert_eq!((top.y, top.height), (100.0, 80.0));
        let end = bar(80.0, 20.0, 100.0).handle(RAIL).unwrap();
        assert_eq!(end.y + end.height, RAIL.y + RAIL.height);
        // A long text still gets a handle big enough to grab.
        let long = bar(0.0, 20.0, 100_000.0).handle(RAIL).unwrap();
        assert_eq!(long.height, MIN_HANDLE);
    }

    #[test]
    fn dragging_the_handle_scrolls_to_where_it_is_dropped() {
        let at_top = bar(0.0, 20.0, 100.0);
        let handle = at_top.handle(RAIL).unwrap();
        // Halfway down its travel is halfway through the rows that can scroll by.
        assert_eq!(at_top.scroll_to(RAIL, handle, 100.0 + 160.0), 40);
        assert_eq!(at_top.scroll_to(RAIL, handle, 9_999.0), 80);
        let at_end = bar(80.0, 20.0, 100.0);
        assert_eq!(at_end.scroll_to(RAIL, handle, 0.0), -80);
    }
}
