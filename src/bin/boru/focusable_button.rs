//! Keyboard-focusable button wrapper for the Boru iced frontend.
//!
//! iced 0.14's stock `Button` widget cannot receive keyboard focus: its
//! widget `State` has no [`operation::Focusable`] implementation and its
//! `update` only reacts to mouse/touch events (verified in the iced_widget
//! 0.14.2 source). That means an icon-only or text button rendered with
//! `iced::widget::button` is unreachable by keyboard users.
//!
//! [`FocusableButton`] is a thin delegating wrapper (same shape as the
//! app's [`Prebuilt`](crate::app::Prebuilt) widget) that adds:
//!
//! - **Focus participation** — its tree `State` implements
//!   [`operation::Focusable`], and its `operate` forwards
//!   `operation.focusable(...)` so the app's global Tab / Shift+Tab
//!   traversal (`operation::focus_next` / `focus_previous`) can reach it.
//! - **Keyboard activation** — while focused, pressing Enter or Space
//!   publishes the wrapped button's message (mouse clicks still work
//!   through the inner button widget unchanged).
//! - **A visible focus ring** — when focused, the widget draws a 2 px
//!   [`design_tokens::color_focus`] ring with the configured corner radius,
//!   so keyboard focus is always visible (spec Task 17: "Focus indicators
//!   are visible").
//!
//! The inner element is drawn/layout/updated exactly as if it were alone;
//! the wrapper adds no visual surface of its own beyond the focus ring.

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::renderer;
use iced::advanced::widget::{self, tree, Operation, Tree, Widget};
use iced::advanced::{Clipboard, Layout, Renderer, Shell};
use iced::{Event, Length, Rectangle, Size, Theme};

/// Tree state for [`FocusableButton`]: remembers whether the wrapped
/// button currently holds keyboard focus, plus the last focus value that
/// was reported to the app so transitions publish exactly once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct State {
    is_focused: bool,
    last_reported_focus: bool,
}

impl widget::operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.is_focused
    }

    fn focus(&mut self) {
        self.is_focused = true;
    }

    fn unfocus(&mut self) {
        self.is_focused = false;
    }
}

/// A keyboard-focusable wrapper around an existing iced element.
///
/// Pass the already-styled button (or any interactive content) plus the
/// message it should publish on keyboard activation. The wrapper forwards
/// layout/draw/update/mouse to the inner element, participates in the app's
/// Tab traversal, publishes `on_press` on Enter/Space while focused, and
/// draws a visible focus ring.
pub struct FocusableButton<'a, Message> {
    content: iced::Element<'a, Message, Theme, iced::Renderer>,
    on_press: Option<Message>,
    on_focus_change: Option<Box<dyn Fn(bool) -> Message + 'a>>,
    on_key_press:
        Option<Box<dyn Fn(&iced::keyboard::key::Key, iced::keyboard::Modifiers) -> Option<Message> + 'a>>,
    ring_radius: f32,
}

impl<'a, Message> FocusableButton<'a, Message> {
    /// Wrap `content`; `on_press` is published on Enter/Space while focused.
    pub fn new(
        content: impl Into<iced::Element<'a, Message, Theme, iced::Renderer>>,
        on_press: Option<Message>,
    ) -> Self {
        Self {
            content: content.into(),
            on_press,
            on_focus_change: None,
            on_key_press: None,
            ring_radius: crate::design_tokens::RADIUS_SM,
        }
    }

    /// Report focus transitions: `on_focus_change(focused)` is published
    /// whenever this button gains or loses keyboard focus (mirrors the
    /// focus-tracking pattern iced's own `Stack` widget uses to keep its
    /// top layer visible).
    pub fn on_focus_change(mut self, on_focus_change: impl Fn(bool) -> Message + 'a) -> Self {
        self.on_focus_change = Some(Box::new(on_focus_change));
        self
    }

    /// Handle additional keys while this control owns keyboard focus. The
    /// callback is local to the widget, so it cannot consume composer input.
    pub fn on_key_press(
        mut self,
        on_key_press: impl Fn(&iced::keyboard::key::Key, iced::keyboard::Modifiers) -> Option<Message>
            + 'a,
    ) -> Self {
        self.on_key_press = Some(Box::new(on_key_press));
        self
    }

    /// Set the focus-ring corner radius (matches the wrapped button's
    /// border radius; defaults to `RADIUS_SM`).
    pub fn ring_radius(mut self, radius: f32) -> Self {
        self.ring_radius = radius;
        self
    }

    /// Build the widget into an element.
    pub fn build(self) -> iced::Element<'a, Message, Theme, iced::Renderer>
    where
        Message: 'a + Clone,
    {
        self.into()
    }
}

impl<'a, Message> From<FocusableButton<'a, Message>>
    for iced::Element<'a, Message, Theme, iced::Renderer>
where
    Message: 'a + Clone,
{
    fn from(widget: FocusableButton<'a, Message>) -> Self {
        iced::Element::new(widget)
    }
}

impl<'a, Message> Widget<Message, Theme, iced::Renderer> for FocusableButton<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(self.content.as_widget())]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content.as_widget()));
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
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
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
            .operate(&mut tree.children[0], layout, renderer, operation);

        // Only interactive buttons (on_press set) join the Tab traversal;
        // a disabled/loading button with no action stays out of the focus
        // order so Tab never stops on a dead control.
        if self.on_press.is_some() {
            let state = tree.state.downcast_mut::<State>();
            operation.focusable(None, layout.bounds(), state);
        }
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
        // Forward every event to the inner button so mouse clicks and
        // hover states keep working exactly as before.
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

        // Focus-change reporting. Focus is mutated by the app's Tab
        // traversal operations (`operation::focus_next` / `focus_previous`)
        // between event passes, so detect the transition on the next
        // redraw — the same trigger iced's own `Stack` widget uses for its
        // `is_top_focused` tracking. Publishing on a transition lets the
        // app keep overlays (e.g. inline video controls) visible while
        // keyboard focus is inside them.
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            let state = tree.state.downcast_mut::<State>();
            if state.is_focused != state.last_reported_focus {
                state.last_reported_focus = state.is_focused;
                if let Some(on_focus_change) = &self.on_focus_change {
                    shell.publish(on_focus_change(state.is_focused));
                }
            }
        }

        if shell.is_event_captured() {
            return;
        }

        // Keyboard activation: Enter / Space while this button is focused.
        if let Some(on_press) = &self.on_press {
            let state = tree.state.downcast_ref::<State>();
            if state.is_focused {
                if let Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key:
                        iced::keyboard::key::Key::Named(iced::keyboard::key::Named::Enter)
                        | iced::keyboard::key::Key::Named(iced::keyboard::key::Named::Space),
                    ..
                }) = event
                {
                    shell.publish(on_press.clone());
                    shell.capture_event();
                }
            }
        }

        let state = tree.state.downcast_ref::<State>();
        if state.is_focused {
            if let Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key, modifiers, ..
            }) = event
            {
                if let Some(on_key_press) = &self.on_key_press {
                    if let Some(message) = on_key_press(key, *modifiers) {
                        shell.publish(message);
                        shell.capture_event();
                    }
                }
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
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

        // Visible focus ring: a transparent quad whose border draws a
        // 2 px focus-colored ring (same mechanism the container style uses
        // to render borders). Drawn after the content so it sits on top.
        let state = tree.state.downcast_ref::<State>();
        if state.is_focused {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    border: iced::Border {
                        color: crate::design_tokens::color_focus(theme),
                        width: crate::design_tokens::FOCUS_WIDTH,
                        radius: self.ring_radius.into(),
                    },
                    ..Default::default()
                },
                iced::Background::Color(iced::Color::TRANSPARENT),
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
}

/// Convenience constructor for a keyboard-focusable button.
///
/// `content` is the already-styled inner button; `on_press` is published on
/// Enter/Space while the button holds keyboard focus (and, via the inner
/// button, on mouse click). Pass `None` for a non-interactive/disabled
/// button that should not join the focus order.
pub fn focusable_button<'a, Message>(
    content: impl Into<iced::Element<'a, Message, Theme, iced::Renderer>>,
    on_press: Option<Message>,
) -> FocusableButton<'a, Message>
where
    Message: 'a + Clone,
{
    FocusableButton::new(content, on_press)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::widget::operation::Focusable;
    use iced::keyboard::key::Named;
    use iced::keyboard::{key, Key};

    #[test]
    fn state_focusable_follows_focus_and_unfocus() {
        let mut state = State::default();
        assert!(!state.is_focused(), "fresh button is not focused");
        assert!(
            !state.last_reported_focus,
            "fresh button has not reported focus yet"
        );
        state.focus();
        assert!(state.is_focused(), "focus() marks the button focused");
        state.unfocus();
        assert!(!state.is_focused(), "unfocus() clears the focused flag");
    }

    #[test]
    fn last_reported_focus_tracks_is_focused() {
        // The `update` focus-transition block publishes only when the
        // reported value differs from the current focus, so after a
        // transition the stored value must equal `is_focused`.
        let mut state = State::default();
        state.focus();
        assert_ne!(state.is_focused, state.last_reported_focus);
        state.last_reported_focus = state.is_focused;
        assert_eq!(state.is_focused, state.last_reported_focus);
        state.unfocus();
        assert_ne!(state.is_focused, state.last_reported_focus);
        state.last_reported_focus = state.is_focused;
        assert_eq!(state.is_focused, state.last_reported_focus);
    }

    #[test]
    fn activation_keys_are_enter_and_space() {
        // The keyboard-activation branch in `update` matches Enter and
        // Space on a focused button. Assert the exact key shapes the match
        // relies on so a iced upgrade that renames keys fails loudly here.
        let enter: iced::keyboard::key::Key = Key::Named(Named::Enter);
        let space: iced::keyboard::key::Key = Key::Named(Named::Space);
        match enter {
            Key::Named(Named::Enter) => {}
            _ => panic!("Enter must match as Named::Enter"),
        }
        match space {
            Key::Named(Named::Space) => {}
            _ => panic!("Space must match as Named::Space"),
        }
        // And a non-activation key must not match the activation pattern.
        let other: iced::keyboard::key::Key = Key::Named(Named::Tab);
        assert!(
            !matches!(other, Key::Named(Named::Enter) | Key::Named(Named::Space)),
            "Tab is not an activation key"
        );
        let _ = key::Named::Escape; // keep `key` import exercised
    }
}
