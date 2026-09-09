//! Reveal keyboard focus only inside Discover's single scrollable.
use super::AppMessage;
use iced::advanced::widget::{operation, Id, Operation};
use iced::{Rectangle, Vector};

pub(crate) fn reveal_focus() -> iced::Task<AppMessage> {
    iced::advanced::widget::operate(RevealFocus::default()).discard()
}

#[derive(Default)]
pub(super) struct RevealFocus {
    pending: Option<(Rectangle, Vector)>,
    scope: Option<(Rectangle, Vector)>,
    pub(super) focused: Option<Rectangle>,
    pub(super) viewport: Option<Rectangle>,
    pub(super) translation: Vector,
    offset: Option<f32>,
}

impl Operation for RevealFocus {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
        let previous = self.scope;
        if let Some(scope) = self.pending.take() {
            self.scope = Some(scope);
        }
        visit(self);
        self.scope = previous;
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _: Rectangle,
        translation: Vector,
        _: &mut dyn operation::Scrollable,
    ) {
        if id == Some(&Id::new("discover-page")) {
            self.pending = Some((bounds, translation));
            self.viewport = Some(bounds);
            self.translation = translation;
        }
    }

    fn focusable(
        &mut self,
        _: Option<&Id>,
        bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        if !state.is_focused() {
            return;
        }
        if let Some((viewport, translation)) = self.scope {
            self.focused = Some(bounds);
            let top = bounds.y - viewport.y;
            let bottom = top + bounds.height;
            // Scrollable rounds its translation. Leave room for the outline
            // and fractional logical coordinates instead of clipping an edge.
            self.offset = if top < translation.y {
                Some((top - 4.0).max(0.0))
            } else if bottom > translation.y + viewport.height {
                Some((bottom + 4.0 - viewport.height).min(top).max(0.0))
            } else {
                None
            };
        }
    }

    fn finish(&self) -> operation::Outcome<()> {
        match self.offset {
            Some(y) => operation::Outcome::Chain(Box::new(operation::scrollable::scroll_to(
                Id::new("discover-page"),
                operation::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(y),
                },
            ))),
            None => operation::Outcome::None,
        }
    }
}
