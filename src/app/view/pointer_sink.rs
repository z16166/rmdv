//! Eats pointer moves while the Settings font menu is open.
//!
//! The menu is an Iced overlay. A move outside its bounds is not captured
//! there, so it falls through to the settings rows and their `on_enter`
//! moves the cursor. Capturing the move from this layer would also drop the
//! overlay (Iced dismisses an overlay when the base tree captures the
//! event), so the move is simply not forwarded. Presses and the wheel still
//! are: a press outside the menu closes it.

use crate::app::Message;
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(super) struct PointerSink<'a> {
    content: Element<'a, Message>,
}

impl<'a> PointerSink<'a> {
    pub(super) fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

fn reaches_content(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(
            mouse::Event::ButtonPressed(_)
                | mouse::Event::ButtonReleased(_)
                | mouse::Event::WheelScrolled { .. }
        )
    )
}

impl<'a> Widget<Message, iced::Theme, iced::Renderer> for PointerSink<'a> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // A move is not forwarded and not captured. Forwarding it retargets
        // the row under the pointer; capturing it closes the menu.
        if matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. })) {
            return;
        }
        let cursor = if reaches_content(event) {
            cursor
        } else {
            // Redraws carry the latest cursor. Handing that through would
            // still fire `on_enter` on the row the pointer has moved onto.
            mouse::Cursor::Unavailable
        };
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // The open menu's overlay supplies the pointer while it is hovered.
        // Everywhere else the page stays neutral.
        mouse::Interaction::default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

impl<'a> From<PointerSink<'a>> for Element<'a, Message> {
    fn from(sink: PointerSink<'a>) -> Self {
        Element::new(sink)
    }
}
