//! Viewer-facing session chrome and lifecycle/resource projections.
//!
//! The media decoder intentionally knows nothing about presentation or room
//! policy. These small, data-only projections give the UI a stable source for
//! identity, connection state, negotiated quality, and viewer counts while
//! keeping no media payloads. They also make the no-viewer policy explicit so a
//! host can release capture/encoder/audio resources as soon as the last viewer
//! leaves.

#![allow(missing_docs)]

use super::{presets::QualityPreset, transport::PathKind, ScreenShareSessionId};
use iroh::PublicKey;

/// Connection state shown beside the viewer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerConnectionState {
    Connecting,
    Streaming,
    Reconnecting,
    Ended,
}

/// Sanitized metadata for one viewer; never contains frame or audio data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerChrome {
    pub session_id: ScreenShareSessionId,
    pub peer_id: PublicKey,
    pub display_name: String,
    pub connection: ViewerConnectionState,
    pub path: PathKind,
    pub preset: QualityPreset,
    pub negotiated_width: u32,
    pub negotiated_height: u32,
    pub negotiated_fps: u32,
}

impl ViewerChrome {
    pub fn new(
        session_id: ScreenShareSessionId,
        peer_id: PublicKey,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            peer_id,
            display_name: display_name.into(),
            connection: ViewerConnectionState::Connecting,
            path: PathKind::Unknown,
            preset: QualityPreset::Balanced,
            negotiated_width: 0,
            negotiated_height: 0,
            negotiated_fps: 0,
        }
    }

    pub fn viewer_count(registry: &ViewerRegistry) -> usize { registry.active_count() }
}

/// Bounded viewer registry for a host session. A stale reconnect cannot leave
/// an unbounded map behind, and ended viewers are removed immediately.
#[derive(Debug)]
pub struct ViewerRegistry {
    viewers: Vec<ViewerChrome>,
    max_viewers: usize,
}

impl ViewerRegistry {
    pub const DEFAULT_MAX_VIEWERS: usize = 8;

    pub fn new(max_viewers: usize) -> Self {
        Self { viewers: Vec::new(), max_viewers: max_viewers.max(1) }
    }

    pub fn upsert(&mut self, viewer: ViewerChrome) -> bool {
        if let Some(existing) = self.viewers.iter_mut().find(|v| v.session_id == viewer.session_id) {
            *existing = viewer;
            return true;
        }
        if self.viewers.len() >= self.max_viewers { return false; }
        self.viewers.push(viewer);
        true
    }

    pub fn remove(&mut self, session_id: ScreenShareSessionId) -> Option<ViewerChrome> {
        let index = self.viewers.iter().position(|v| v.session_id == session_id)?;
        Some(self.viewers.swap_remove(index))
    }

    pub fn get(&self, session_id: ScreenShareSessionId) -> Option<&ViewerChrome> {
        self.viewers.iter().find(|v| v.session_id == session_id)
    }

    pub fn active_count(&self) -> usize {
        self.viewers.iter().filter(|v| v.connection != ViewerConnectionState::Ended).count()
    }

    pub fn len(&self) -> usize { self.viewers.len() }
    pub fn is_empty(&self) -> bool { self.viewers.is_empty() }
    pub fn viewers(&self) -> &[ViewerChrome] { &self.viewers }
}

impl Default for ViewerRegistry {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_VIEWERS)
    }
}

/// Resource action to apply after a viewer-count transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerResourceAction {
    pub capture: bool,
    pub encoder: bool,
    pub audio: bool,
}

impl ViewerResourceAction {
    /// Keep the pipeline alive only while at least one viewer is active.
    pub fn for_viewer_count(viewer_count: usize) -> Self {
        let active = viewer_count != 0;
        Self { capture: active, encoder: active, audio: active }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer() -> ViewerChrome {
        ViewerChrome::new(ScreenShareSessionId::from_bytes([7; 16]), iroh::SecretKey::generate().public(), "Alice")
    }

    #[test]
    fn registry_is_bounded_and_reconnect_updates_identity() {
        let mut registry = ViewerRegistry::new(1);
        assert!(registry.upsert(viewer()));
        let mut replacement = viewer();
        replacement.connection = ViewerConnectionState::Reconnecting;
        assert!(registry.upsert(replacement));
        assert_eq!(registry.active_count(), 1);
        let second = ViewerChrome::new(ScreenShareSessionId::from_bytes([8; 16]), iroh::SecretKey::generate().public(), "Bob");
        assert!(!registry.upsert(second));
    }

    #[test]
    fn default_registry_accepts_a_viewer() {
        let mut registry = ViewerRegistry::default();
        assert!(registry.upsert(viewer()));
        assert_eq!(registry.active_count(), 1);
    }

    #[test]
    fn ended_viewer_does_not_keep_resources_alive() {
        let mut registry = ViewerRegistry::new(2);
        assert!(registry.upsert(viewer()));
        let id = viewer().session_id;
        let mut ended = viewer();
        ended.connection = ViewerConnectionState::Ended;
        assert!(registry.upsert(ended));
        assert_eq!(registry.active_count(), 0);
        assert_eq!(ViewerResourceAction::for_viewer_count(registry.active_count()), ViewerResourceAction { capture: false, encoder: false, audio: false });
        assert!(registry.remove(id).is_some());
    }
}
