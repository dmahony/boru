//! Runtime metadata for the developer-only visual layout designer.
//!
//! This is a projection of the typed [`LayoutConfig`](crate::layout::LayoutConfig),
//! not another persistence format. The view tree may provide an optional current
//! [`Bounds`] snapshot, while editable values and capabilities come from the
//! same layout fields that are read and written by `boru-layout.toml`.

use std::collections::BTreeMap;

use crate::designer::ComponentId;
use crate::layout::LayoutConfig;

/// A logical, responsive bounds snapshot supplied by the view layer.
///
/// Coordinates are intentionally optional and transient. They are useful for
/// selection handles in the running designer, but are never serialized into a
/// layout file or used as the layout model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn from_rectangle(rectangle: iced::Rectangle) -> Self {
        Self::new(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
    }
}

/// An operation supported by the typed layout model for a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayoutOperation {
    Reorder,
    ResizeWidth,
    ResizeHeight,
    ChangeMode,
    ChangeColumns,
    ChangeOrientation,
    Visibility,
    Alignment,
    Spacing,
}

/// A numeric constraint advertised to resize controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Constraint {
    pub min: Option<f32>,
    pub max: Option<f32>,
}

impl Constraint {
    pub const fn range(min: f32, max: f32) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
        }
    }

    pub const fn at_least(min: f32) -> Self {
        Self {
            min: Some(min),
            max: None,
        }
    }
}

/// Runtime description of one editable component.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentMeta {
    pub component_id: ComponentId,
    pub parent_layout_id: &'static str,
    pub current_bounds: Option<Bounds>,
    pub allowed_operations: Vec<LayoutOperation>,
    /// Human-readable values keyed by the corresponding `LayoutConfig` path.
    /// These are display metadata; edits must go through the typed override
    /// model rather than this map.
    pub layout_properties: BTreeMap<&'static str, String>,
    /// Width/height constraints for the resize affordances.
    pub constraints: BTreeMap<&'static str, Constraint>,
}

impl ComponentMeta {
    fn new(
        component_id: ComponentId,
        parent_layout_id: &'static str,
        current_bounds: Option<Bounds>,
        allowed_operations: &[LayoutOperation],
    ) -> Self {
        Self {
            component_id,
            parent_layout_id,
            current_bounds,
            allowed_operations: allowed_operations.to_vec(),
            layout_properties: BTreeMap::new(),
            constraints: BTreeMap::new(),
        }
    }

    fn property(mut self, path: &'static str, value: impl ToString) -> Self {
        self.layout_properties.insert(path, value.to_string());
        self
    }

    fn constraint(mut self, axis: &'static str, value: Constraint) -> Self {
        self.constraints.insert(axis, value);
        self
    }
}

/// Runtime registry view over the currently active typed layout.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutMetadata {
    components: Vec<ComponentMeta>,
}

impl LayoutMetadata {
    /// Build metadata for all stable designer component IDs.
    pub fn from_layout(layout: &LayoutConfig) -> Self {
        Self::with_bounds(layout, |_| None)
    }

    /// Build metadata while attaching transient bounds obtained from the view.
    pub fn with_bounds<F>(layout: &LayoutConfig, mut bounds: F) -> Self
    where
        F: FnMut(ComponentId) -> Option<Bounds>,
    {
        Self {
            components: ComponentId::ALL
                .into_iter()
                .map(|id| component_metadata(layout, id, bounds(id)))
                .collect(),
        }
    }

    pub fn components(&self) -> &[ComponentMeta] {
        &self.components
    }

    pub fn get(&self, component_id: ComponentId) -> Option<&ComponentMeta> {
        self.components
            .iter()
            .find(|meta| meta.component_id == component_id)
    }
}

/// Obtain metadata for one component from the active typed layout.
pub fn metadata_for(
    layout: &LayoutConfig,
    component_id: ComponentId,
    current_bounds: Option<Bounds>,
) -> ComponentMeta {
    component_metadata(layout, component_id, current_bounds)
}

fn component_metadata(
    layout: &LayoutConfig,
    id: ComponentId,
    current_bounds: Option<Bounds>,
) -> ComponentMeta {
    use LayoutOperation::*;

    match id {
        ComponentId::HomeWelcome => {
            ComponentMeta::new(id, "home", current_bounds, &[Reorder, Visibility])
                .property(
                    "home.section_order",
                    format!("{:?}", layout.home.section_order),
                )
                .property(
                    "home.hidden_sections",
                    format!("{:?}", layout.home.hidden_sections),
                )
        }
        ComponentId::HomeQuickActions => ComponentMeta::new(
            id,
            "home",
            current_bounds,
            &[Reorder, ChangeMode, ChangeColumns, Visibility, Spacing],
        )
        .property("home.mode", format!("{:?}", layout.home.mode))
        .property(
            "home.quick_actions.columns_wide",
            layout.home.quick_actions.columns_wide,
        )
        .property(
            "home.quick_actions.columns_mid",
            layout.home.quick_actions.columns_mid,
        )
        .property(
            "home.quick_actions.columns_narrow",
            layout.home.quick_actions.columns_narrow,
        )
        .property("home.gaps", format!("{:?}", layout.home.gaps)),
        ComponentId::HomePublicRooms => {
            ComponentMeta::new(id, "home", current_bounds, &[Reorder, Visibility])
                .property(
                    "home.section_order",
                    format!("{:?}", layout.home.section_order),
                )
                .property(
                    "home.hidden_sections",
                    format!("{:?}", layout.home.hidden_sections),
                )
        }
        ComponentId::HomeFriends => {
            ComponentMeta::new(id, "home", current_bounds, &[Reorder, Visibility])
                .property(
                    "home.section_order",
                    format!("{:?}", layout.home.section_order),
                )
                .property(
                    "home.hidden_sections",
                    format!("{:?}", layout.home.hidden_sections),
                )
        }
        ComponentId::HomeRecentActivity => {
            ComponentMeta::new(id, "home", current_bounds, &[Reorder, Visibility])
                .property(
                    "home.section_order",
                    format!("{:?}", layout.home.section_order),
                )
                .property(
                    "home.hidden_sections",
                    format!("{:?}", layout.home.hidden_sections),
                )
        }
        ComponentId::Sidebar => ComponentMeta::new(
            id,
            "sidebar",
            current_bounds,
            &[ResizeWidth, Reorder, Visibility, Spacing],
        )
        .property("sidebar.width", layout.sidebar.width)
        .property(
            "sidebar.section_order",
            format!("{:?}", layout.sidebar.section_order),
        )
        .property("sidebar.padding", format!("{:?}", layout.sidebar.padding))
        .constraint(
            "width",
            Constraint::range(layout.sidebar.width_min, layout.sidebar.width_max),
        ),
        ComponentId::ChatMessageList => ComponentMeta::new(
            id,
            "chat",
            current_bounds,
            &[ResizeWidth, ResizeHeight, Alignment, Spacing],
        )
        .property("chat.bubble_max_width", layout.chat.bubble_max_width)
        .property("chat.bubble_width_ratio", layout.chat.bubble_width_ratio)
        .property("chat.message_max_width", layout.chat.message_max_width)
        .constraint("width", Constraint::at_least(1.0)),
        ComponentId::ChatComposer => ComponentMeta::new(
            id,
            "chat",
            current_bounds,
            &[Reorder, ResizeWidth, ChangeOrientation, Alignment, Spacing],
        )
        .property(
            "chat.composer.button_order",
            format!("{:?}", layout.chat.composer.button_order),
        )
        .property("chat.composer.spacing", layout.chat.composer.spacing)
        .property("chat.composer.padding", layout.chat.composer.padding)
        .constraint("width", Constraint::at_least(1.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_all_stable_components() {
        let metadata = LayoutMetadata::from_layout(&LayoutConfig::default());
        assert_eq!(metadata.components().len(), ComponentId::ALL.len());
        for id in ComponentId::ALL {
            assert!(metadata.get(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn properties_follow_active_layout_and_bounds_are_transient() {
        let mut layout = LayoutConfig::default();
        layout.sidebar.width = 300.0;
        let bounds = Bounds::new(1.0, 2.0, 300.0, 400.0);
        let meta = metadata_for(&layout, ComponentId::Sidebar, Some(bounds));
        assert_eq!(meta.current_bounds, Some(bounds));
        assert_eq!(meta.layout_properties["sidebar.width"], "300");
        assert_eq!(meta.constraints["width"], Constraint::range(288.0, 320.0));
    }

    #[test]
    fn quick_actions_advertises_supported_mode_editing() {
        let meta = metadata_for(&LayoutConfig::default(), ComponentId::HomeQuickActions, None);
        assert!(meta.allowed_operations.contains(&LayoutOperation::ChangeMode));
        assert_eq!(meta.layout_properties["home.mode"], "Grid");
    }
}
