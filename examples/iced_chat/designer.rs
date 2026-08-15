//! Developer-only state for the visual UI designer.
//!
//! This module deliberately contains only transient editor state. It does not
//! reference (or own) chat, networking, room, media, transfer, or persistence
//! state. The application wraps [`DesignerMessage`] in its normal
//! `AppMessage`, so designer changes use the same Iced update pipeline as all
//! other UI actions.

use crate::layout::HomeSection;
use iced::widget::{button, container, mouse_area, row, text, Stack};
use iced::{Background, Border, Color, Element, Length, Padding, Point};
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

/// Compose the transient outline and semantic label for an editable region.
/// Nested regions can be wrapped independently; iced dispatches pointer events
/// to the most specific child first, keeping hit-testing predictable.
pub(crate) fn overlay<'a>(
    component: ComponentId,
    content: Element<'a, crate::app::AppMessage>,
    enabled: bool,
    hovered: Option<ComponentId>,
    selected: Option<ComponentId>,
    resize_value: Option<f32>,
) -> Element<'a, crate::app::AppMessage> {
    if !enabled {
        return content;
    }
    let active = hovered == Some(component);
    let is_selected = selected == Some(component);
    let label: Element<'a, crate::app::AppMessage> = if active {
        container(text(component.as_str()).size(11.0).color(Color::WHITE))
            .padding(Padding::from(3.0))
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.08, 0.32, 0.72, 0.94))),
                border: Border {
                    color: Color::from_rgb(0.55, 0.78, 1.0),
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .into()
    } else {
        container(text("")).into()
    };
    // Keep the drag target explicit rather than making the whole production
    // card draggable.  The latter is particularly dangerous for cards whose
    // normal click opens a room, starts playback, or downloads a file.  The
    // handle is part of the developer-only overlay, so it cannot alter the
    // normal application surface when Designer Mode is disabled.
    let drag_handle = mouse_area(
        container(text("⠿").size(16.0).color(Color::WHITE))
            .padding(Padding::from(4.0))
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.08, 0.32, 0.72, 0.94))),
                border: Border {
                    color: Color::from_rgb(0.55, 0.78, 1.0),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }),
    )
    .on_press(crate::app::AppMessage::Designer(
        DesignerMessage::StartDrag {
            component,
            origin: Point::ORIGIN,
        },
    ))
    .on_move(|point| crate::app::AppMessage::Designer(DesignerMessage::UpdateDrag(point)))
    .on_release(crate::app::AppMessage::Designer(
        DesignerMessage::CommitDrag,
    ));
    let resize_handle = mouse_area(
        container(text("↘").size(14.0).color(Color::WHITE))
            .padding(Padding::from(3.0))
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.72, 0.32, 0.08, 0.94))),
                border: Border {
                    color: Color::from_rgb(1.0, 0.78, 0.32),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }),
    )
    .on_press(crate::app::AppMessage::Designer(DesignerMessage::StartResize {
        component,
        origin: Point::ORIGIN,
    }))
    .on_move(|point| crate::app::AppMessage::Designer(DesignerMessage::UpdateResize(point)))
    .on_release(crate::app::AppMessage::Designer(DesignerMessage::CancelResize));
    let resize_label = resize_value.map(|value| {
        container(text(format!("{value:.0}px")).size(11.0).color(Color::WHITE))
            .padding(Padding::from(3.0))
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.72, 0.32, 0.08, 0.94))),
                border: Border::default(),
                ..Default::default()
            })
    });
    let layered = Stack::new()
        .push(container(content).style(move |_| container::Style {
            border: if is_selected {
                Border {
                    color: Color::from_rgb(1.0, 0.72, 0.16),
                    width: 3.0,
                    radius: 4.0.into(),
                }
            } else if active {
                Border {
                    color: Color::from_rgb(0.25, 0.68, 1.0),
                    width: 2.0,
                    radius: 4.0.into(),
                }
            } else {
                Border::default()
            },
            ..Default::default()
        }))
        .push(
            container(label)
                .width(Length::Shrink)
                .height(Length::Shrink),
        );
    let layered = Stack::new().push(layered).push(
        container(drag_handle)
            .width(Length::Shrink)
            .height(Length::Shrink),
    );
    // Grid editing is visible only for the selected Quick Actions grid. The
    // app maps these semantic deltas to the active typed layout field.
    let layered = if component == ComponentId::HomeQuickActions && is_selected {
        let grid_controls = row![
            button(text("−").size(14.0))
                .padding([2, 7])
                .on_press(crate::app::AppMessage::Designer(
                    DesignerMessage::AdjustGridColumns(-1),
                )),
            button(text("+").size(14.0))
                .padding([2, 7])
                .on_press(crate::app::AppMessage::Designer(
                    DesignerMessage::AdjustGridColumns(1),
                )),
        ]
        .spacing(2);
        Stack::new().push(layered).push(
            container(grid_controls)
                .width(Length::Shrink)
                .height(Length::Shrink),
        )
    } else {
        layered
    };
    let supports_resize = matches!(
        component,
        ComponentId::Sidebar | ComponentId::ChatMessageList | ComponentId::ChatComposer
    );
    let layered = if supports_resize {
        Stack::new()
            .push(layered)
            .push(
                container(resize_handle)
                    .width(Length::Shrink)
                    .height(Length::Shrink),
            )
            .push(
                container(resize_label.unwrap_or_else(|| container(text(""))))
                    .width(Length::Shrink)
                    .height(Length::Shrink),
            )
    } else {
        layered
    };
    mouse_area(layered)
        .on_enter(crate::app::AppMessage::Designer(DesignerMessage::Hover(
            Some(component),
        )))
        .on_exit(crate::app::AppMessage::Designer(DesignerMessage::Hover(
            None,
        )))
        .on_press(crate::app::AppMessage::Designer(DesignerMessage::Select(
            Some(component),
        )))
        .into()
}

impl ComponentId {
    /// Map stable designer surfaces to semantic home sections.
    pub(crate) const fn home_section(self) -> Option<HomeSection> {
        match self {
            Self::HomeWelcome => Some(HomeSection::Hero),
            Self::HomeQuickActions => Some(HomeSection::QuickActions),
            Self::HomePublicRooms => Some(HomeSection::MeshHealth),
            Self::HomeFriends => Some(HomeSection::PeopleActivity),
            Self::HomeRecentActivity => Some(HomeSection::Tunnels),
            Self::Sidebar | Self::ChatMessageList | Self::ChatComposer => None,
        }
    }

    /// Map a designer surface to the existing inspector's authoritative
    /// component hierarchy. Several fine-grained designer surfaces share the
    /// same theme/layout section.
    pub(crate) fn inspector_component(self) -> crate::inspector::ComponentId {
        match self {
            Self::HomeWelcome
            | Self::HomeQuickActions
            | Self::HomePublicRooms
            | Self::HomeFriends
            | Self::HomeRecentActivity => crate::inspector::ComponentId::Home,
            Self::Sidebar => crate::inspector::ComponentId::Sidebar,
            Self::ChatMessageList | Self::ChatComposer => crate::inspector::ComponentId::Chat,
        }
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
    pub section: Option<HomeSection>,
    pub origin: Point,
    pub current: Point,
    pub proposed_index: Option<usize>,
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
    CommitDrag,
    CancelDrag,
    StartResize {
        component: ComponentId,
        origin: Point,
    },
    UpdateResize(Point),
    CancelResize,
    SetBreakpoint(PreviewBreakpoint),
    /// Increment/decrement the selected grid at the active preview breakpoint.
    AdjustGridColumns(i8),
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
                    section: component.home_section(),
                    origin,
                    current: origin,
                    proposed_index: None,
                });
                self.selected_component = Some(component);
            }
            DesignerMessage::StartDrag { .. } => {}
            DesignerMessage::UpdateDrag(current) => {
                if let Some(operation) = &mut self.drag_operation {
                    operation.current = current;
                }
            }
            DesignerMessage::CommitDrag => self.drag_operation = None,
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
            DesignerMessage::AdjustGridColumns(_) => {}
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

    #[test]
    fn resize_operation_tracks_pointer_until_cancelled() {
        let mut state = DesignerState::default();
        state.update(DesignerMessage::Enter);
        state.update(DesignerMessage::StartResize {
            component: ComponentId::Sidebar,
            origin: Point::new(10.0, 20.0),
        });
        state.update(DesignerMessage::UpdateResize(Point::new(42.0, 24.0)));
        let operation = state.resize_operation.as_ref().expect("resize started");
        assert_eq!(operation.component, ComponentId::Sidebar);
        assert_eq!(operation.current, Point::new(42.0, 24.0));
        state.update(DesignerMessage::CancelResize);
        assert!(state.resize_operation.is_none());
    }
}
