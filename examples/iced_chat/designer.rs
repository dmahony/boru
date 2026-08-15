//! Developer-only state for the visual UI designer.
//!
//! This module deliberately contains only transient editor state. It does not
//! reference (or own) chat, networking, room, media, transfer, or persistence
//! state. The application wraps [`DesignerMessage`] in its normal
//! `AppMessage`, so designer changes use the same Iced update pipeline as all
//! other UI actions.

use iced::Point;
use std::fmt;
use std::str::FromStr;

/// Stable semantic identifiers for the parts of the application exposed to
/// the visual designer. These are part of the designer's file/inspector
/// contract: add new variants, but do not rename or reorder existing ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentId {
    HomeWelcome,
    HomeQuickActions,
    HomePublicRooms,
    HomeFriends,
    HomeRecentActivity,
    Sidebar,
    ChatMessageList,
    ChatComposer,
}

impl ComponentId {
    pub const ALL: [Self; 8] = [
        Self::HomeWelcome,
        Self::HomeQuickActions,
        Self::HomePublicRooms,
        Self::HomeFriends,
        Self::HomeRecentActivity,
        Self::Sidebar,
        Self::ChatMessageList,
        Self::ChatComposer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HomeWelcome => "home.welcome",
            Self::HomeQuickActions => "home.quick_actions",
            Self::HomePublicRooms => "home.public_rooms",
            Self::HomeFriends => "home.friends",
            Self::HomeRecentActivity => "home.recent_activity",
            Self::Sidebar => "sidebar",
            Self::ChatMessageList => "chat.message_list",
            Self::ChatComposer => "chat.composer",
        }
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ComponentId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|id| id.as_str() == value)
            .ok_or_else(|| format!("unknown designer component ID: {value}"))
    }
}

/// Responsive preview bands exposed by the designer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewBreakpoint {
    Compact,
    Medium,
    Reference,
    Large,
}

impl Default for PreviewBreakpoint {
    fn default() -> Self {
        Self::Reference
    }
}

/// A transient drag operation. Coordinates are pointer-session data only; no
/// desktop coordinates are persisted by the designer.
#[derive(Debug, Clone, PartialEq)]
pub struct DragOperation {
    pub component: ComponentId,
    pub origin: Point,
    pub current: Point,
}

/// A transient resize operation. The eventual layout layer will translate the
/// delta into responsive constraints rather than absolute coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct ResizeOperation {
    pub component: ComponentId,
    pub origin: Point,
    pub current: Point,
}

/// State owned by the developer-only visual designer overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignerState {
    pub enabled: bool,
    pub hovered_component: Option<ComponentId>,
    pub selected_component: Option<ComponentId>,
    pub drag_operation: Option<DragOperation>,
    pub resize_operation: Option<ResizeOperation>,
    pub preview_breakpoint: PreviewBreakpoint,
    pub dirty: bool,
    pub validation_errors: Vec<String>,
}

impl Default for DesignerState {
    fn default() -> Self {
        Self {
            enabled: false,
            hovered_component: None,
            selected_component: None,
            drag_operation: None,
            resize_operation: None,
            preview_breakpoint: PreviewBreakpoint::default(),
            dirty: false,
            validation_errors: Vec::new(),
        }
    }
}

/// Messages that mutate only [`DesignerState`].
#[derive(Debug, Clone, PartialEq)]
pub enum DesignerMessage {
    Enter,
    Exit,
    Hover(Option<ComponentId>),
    Select(Option<ComponentId>),
    StartDrag {
        component: ComponentId,
        origin: Point,
    },
    UpdateDrag(Point),
    CancelDrag,
    StartResize {
        component: ComponentId,
        origin: Point,
    },
    UpdateResize(Point),
    CancelResize,
    SetBreakpoint(PreviewBreakpoint),
    MarkDirty,
    SetValidationErrors(Vec<String>),
    ClearValidationErrors,
}

impl DesignerState {
    /// Apply one designer message. Keeping this reducer independent makes the
    /// state split explicit and keeps `IcedChat::update` as the sole routing
    /// boundary for designer actions.
    pub fn update(&mut self, message: DesignerMessage) {
        match message {
            DesignerMessage::Enter => self.enabled = true,
            DesignerMessage::Exit => {
                self.enabled = false;
                self.hovered_component = None;
                self.selected_component = None;
                self.drag_operation = None;
                self.resize_operation = None;
            }
            DesignerMessage::Hover(component) => self.hovered_component = component,
            DesignerMessage::Select(component) => self.selected_component = component,
            DesignerMessage::StartDrag { component, origin } if self.enabled => {
                self.drag_operation = Some(DragOperation {
                    component,
                    origin,
                    current: origin,
                });
            }
            DesignerMessage::StartDrag { .. } => {}
            DesignerMessage::UpdateDrag(current) => {
                if let Some(operation) = &mut self.drag_operation {
                    operation.current = current;
                }
            }
            DesignerMessage::CancelDrag => self.drag_operation = None,
            DesignerMessage::StartResize { component, origin } if self.enabled => {
                self.resize_operation = Some(ResizeOperation {
                    component,
                    origin,
                    current: origin,
                });
            }
            DesignerMessage::StartResize { .. } => {}
            DesignerMessage::UpdateResize(current) => {
                if let Some(operation) = &mut self.resize_operation {
                    operation.current = current;
                }
            }
            DesignerMessage::CancelResize => self.resize_operation = None,
            DesignerMessage::SetBreakpoint(breakpoint) => self.preview_breakpoint = breakpoint,
            DesignerMessage::MarkDirty => self.dirty = true,
            DesignerMessage::SetValidationErrors(errors) => self.validation_errors = errors,
            DesignerMessage::ClearValidationErrors => self.validation_errors.clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_disabled_and_empty() {
        let state = DesignerState::default();
        assert!(!state.enabled);
        assert!(state.hovered_component.is_none());
        assert!(state.selected_component.is_none());
        assert!(state.drag_operation.is_none());
        assert!(state.resize_operation.is_none());
        assert!(!state.dirty);
        assert!(state.validation_errors.is_empty());
    }

    #[test]
    fn exit_cancels_transient_interaction_state() {
        let mut state = DesignerState::default();
        state.update(DesignerMessage::Enter);
        state.update(DesignerMessage::Select(Some(ComponentId::HomeWelcome)));
        state.update(DesignerMessage::StartDrag {
            component: ComponentId::HomeWelcome,
            origin: Point::new(1.0, 2.0),
        });
        state.update(DesignerMessage::Exit);
        assert!(!state.enabled);
        assert!(state.selected_component.is_none());
        assert!(state.drag_operation.is_none());
    }

    #[test]
    fn component_ids_are_stable_and_round_trip() {
        let expected = [
            "home.welcome",
            "home.quick_actions",
            "home.public_rooms",
            "home.friends",
            "home.recent_activity",
            "sidebar",
            "chat.message_list",
            "chat.composer",
        ];
        assert_eq!(ComponentId::ALL.map(ComponentId::as_str), expected);
        for id in ComponentId::ALL {
            assert_eq!(id.as_str().parse::<ComponentId>().unwrap(), id);
        }
    }
}
