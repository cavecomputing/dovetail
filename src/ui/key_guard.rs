//! Keeps the app's Ctrl+digit and Ctrl+punctuation shortcuts (Ctrl+=, Ctrl+-, Ctrl+0, Ctrl+,)
//! from typing into text boxes. iced's text inputs type a key's character whatever the
//! modifiers; Ctrl+letter comes as a control character they skip, but Ctrl+= still comes as "=".
//! Wrapped around a whole window, this takes those key presses before any widget sees them. The
//! app-wide shortcut handler still gets them (`event::listen_with` sees captured events too).

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, Widget};
use iced::advanced::{Clipboard, Shell, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector, keyboard};

use crate::ui::app::Message;

pub struct KeyGuard<'a> {
    content: Element<'a, Message>,
}

/// `content`, with Ctrl+digit and Ctrl+punctuation kept from its text boxes.
pub fn guard<'a>(content: impl Into<Element<'a, Message>>) -> KeyGuard<'a> {
    KeyGuard {
        content: content.into(),
    }
}

/// Whether `event` is a shortcut whose character a text box would type: Ctrl (Cmd on macOS)
/// with a key that isn't a letter.
fn types_its_character(event: &Event) -> bool {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            let character = match key.as_ref() {
                keyboard::Key::Character(text) => text.chars().next(),
                _ => None,
            };
            modifiers.command() && character.is_some_and(|c| !c.is_alphabetic())
        }
        _ => false,
    }
}

impl Widget<Message, Theme, Renderer> for KeyGuard<'_> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if types_its_character(event) {
            shell.capture_event();
            return;
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a> From<KeyGuard<'a>> for Element<'a, Message> {
    fn from(guard: KeyGuard<'a>) -> Self {
        Element::new(guard)
    }
}
