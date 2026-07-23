//! Window focus and visibility state tracking.
//!
//! This module provides [`WindowFocusTracker`], which maintains a reliable
//! source of truth for the application's window state (focused, visible,
//! minimised) and exposes it to the notification service.
//!
//! # Architecture
//!
//! Focus state is updated from real Iced window events:
//!
//! - `iced::window::focus_events()` — Focused / Unfocused
//! - `iced::window::mode_events()` — Minimized / Normal / Fullscreen
//! - Conversation switches are tracked from `AppMessage` handlers
//!
//! The tracker also provides Iced [`Subscription`]s that emit
//! [`AppMessage::WindowFocusEvent`] variants so the app's update() method can
//! forward changes to the notification service.

use crate::notification::service::FocusState;
use std::time::{Duration, Instant};

// ── Window focus tracker ───────────────────────────────────────────────────

/// Reliable source of truth for application window visibility.
///
/// Updated from real GUI events, not inferred from conversation selection.
#[derive(Debug, Clone)]
pub struct WindowFocusTracker {
    /// Whether the window currently has keyboard focus.
    pub window_focused: bool,
    /// Whether the window is visible on screen (not minimised or hidden).
    pub window_visible: bool,
    /// Whether the window is minimised.
    pub window_minimised: bool,
    /// Whether the app is running in background (tray/minimise-to-tray) mode.
    pub running_in_background: bool,
    /// The conversation topic currently active in the chat panel, if any.
    pub active_conversation_id: Option<String>,
    /// Timestamp of the last focus transition (for debounce logic).
    last_focus_change: Instant,
    /// Number of focus events received (for diagnostics).
    focus_event_count: u64,
}

impl Default for WindowFocusTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowFocusTracker {
    /// Create a new tracker with the default initial state (focused, visible).
    pub fn new() -> Self {
        Self {
            window_focused: true,
            window_visible: true,
            window_minimised: false,
            running_in_background: false,
            active_conversation_id: None,
            last_focus_change: Instant::now(),
            focus_event_count: 0,
        }
    }

    /// Call when the window gains focus.
    pub fn on_focused(&mut self) -> Vec<(String, String)> {
        let prev = self.window_focused;
        self.window_focused = true;
        self.window_visible = true;
        self.window_minimised = false;
        self.last_focus_change = Instant::now();
        self.focus_event_count += 1;

        let mut log: Vec<(String, String)> = Vec::new();
        if prev != self.window_focused {
            log.push(("focus".into(), "gained".into()));
            tracing::debug!("WindowFocusTracker: focus gained");
        }
        log
    }

    /// Call when the window loses focus.
    pub fn on_unfocused(&mut self) -> Vec<(String, String)> {
        let prev = self.window_focused;
        self.window_focused = false;
        // Window is still visible when unfocused (e.g. another app is overlaid)
        self.window_visible = true;
        self.last_focus_change = Instant::now();
        self.focus_event_count += 1;

        let mut log: Vec<(String, String)> = Vec::new();
        if prev != self.window_focused {
            log.push(("focus".into(), "lost".into()));
            tracing::debug!("WindowFocusTracker: focus lost");
        }
        log
    }

    /// Call when the window is minimised.
    pub fn on_minimised(&mut self) -> Vec<(String, String)> {
        let prev_visible = self.window_visible;
        let prev_min = self.window_minimised;
        self.window_focused = false;
        self.window_visible = false;
        self.window_minimised = true;
        self.last_focus_change = Instant::now();

        let mut log: Vec<(String, String)> = Vec::new();
        if prev_visible != self.window_visible || prev_min != self.window_minimised {
            log.push(("window".into(), "minimised".into()));
            tracing::debug!("WindowFocusTracker: window minimised");
        }
        log
    }

    /// Call when the window is restored from minimised state.
    pub fn on_restored(&mut self) -> Vec<(String, String)> {
        let prev_visible = self.window_visible;
        let prev_min = self.window_minimised;
        self.window_visible = true;
        self.window_minimised = false;
        // Focus state may still be tracked — don't reset it here
        self.last_focus_change = Instant::now();

        let mut log: Vec<(String, String)> = Vec::new();
        if prev_visible != self.window_visible || prev_min != self.window_minimised {
            log.push(("window".into(), "restored".into()));
            tracing::debug!("WindowFocusTracker: window restored from minimised");
        }
        log
    }

    /// Call when the user switches to a different conversation.
    pub fn on_conversation_changed(&mut self, topic: Option<String>) {
        if self.active_conversation_id != topic {
            tracing::debug!(
                old = ?self.active_conversation_id,
                new = ?topic,
                "WindowFocusTracker: conversation changed"
            );
            self.active_conversation_id = topic;
        }
    }

    /// Call when the app enters or exits background (tray) mode.
    pub fn set_running_in_background(&mut self, bg: bool) {
        if self.running_in_background != bg {
            tracing::debug!("WindowFocusTracker: background mode = {bg}");
            self.running_in_background = bg;
            if bg {
                self.window_visible = false;
                self.window_focused = false;
            }
        }
    }

    /// Build a [`FocusState`] snapshot for the notification service.
    pub fn to_focus_state(&self) -> FocusState {
        FocusState {
            window_focused: self.window_focused,
            window_visible: self.window_visible,
            active_conversation_id: self.active_conversation_id.clone(),
            running_in_background: self.running_in_background,
        }
    }

    /// Total focus events tracked (for diagnostics).
    pub fn focus_event_count(&self) -> u64 {
        self.focus_event_count
    }

    /// Time since last focus change.
    pub fn time_since_last_change(&self) -> Duration {
        Instant::now().duration_since(self.last_focus_change)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let tracker = WindowFocusTracker::new();
        assert!(tracker.window_focused);
        assert!(tracker.window_visible);
        assert!(!tracker.window_minimised);
        assert!(!tracker.running_in_background);
        assert_eq!(tracker.active_conversation_id, None);
    }

    #[test]
    fn test_focus_gained() {
        let mut tracker = WindowFocusTracker::new();
        tracker.window_focused = false; // Simulate initial unfocused state
        let log = tracker.on_focused();
        assert!(tracker.window_focused);
        assert!(!log.is_empty());
    }

    #[test]
    fn test_focus_lost() {
        let mut tracker = WindowFocusTracker::new();
        let log = tracker.on_unfocused();
        assert!(!tracker.window_focused);
        assert!(tracker.window_visible); // Window still visible when unfocused
        assert!(!log.is_empty());
    }

    #[test]
    fn test_minimise_and_restore() {
        let mut tracker = WindowFocusTracker::new();

        // Minimize
        let log = tracker.on_minimised();
        assert!(!tracker.window_focused);
        assert!(!tracker.window_visible);
        assert!(tracker.window_minimised);
        assert!(!log.is_empty());

        // Restore
        let log = tracker.on_restored();
        assert!(tracker.window_visible);
        assert!(!tracker.window_minimised);
        assert!(!log.is_empty());
    }

    #[test]
    fn test_conversation_change() {
        let mut tracker = WindowFocusTracker::new();
        assert_eq!(tracker.active_conversation_id, None);

        tracker.on_conversation_changed(Some("topic-abc".into()));
        assert_eq!(tracker.active_conversation_id, Some("topic-abc".into()));

        tracker.on_conversation_changed(None);
        assert_eq!(tracker.active_conversation_id, None);
    }

    #[test]
    fn test_to_focus_state() {
        let mut tracker = WindowFocusTracker::new();
        tracker.on_unfocused();
        tracker.on_conversation_changed(Some("conv-1".into()));

        let state = tracker.to_focus_state();
        assert!(!state.window_focused);
        assert!(state.window_visible);
        assert_eq!(state.active_conversation_id, Some("conv-1".into()));
    }

    #[test]
    fn test_background_mode() {
        let mut tracker = WindowFocusTracker::new();

        tracker.set_running_in_background(true);
        assert!(tracker.running_in_background);
        assert!(!tracker.window_focused);
        assert!(!tracker.window_visible);

        tracker.set_running_in_background(false);
        assert!(!tracker.running_in_background);
    }

    #[test]
    fn test_noop_setters_dont_log() {
        let mut tracker = WindowFocusTracker::new();
        // Calling on_focused when already focused should not log
        let log = tracker.on_focused();
        assert!(log.is_empty());
    }

    #[test]
    fn test_conversation_noop_no_log() {
        let mut tracker = WindowFocusTracker::new();
        tracker.on_conversation_changed(Some("same".into()));
        // Same value again — no log expected (can't verify log output directly,
        // but at least shouldn't panic)
        tracker.on_conversation_changed(Some("same".into()));
    }
}
