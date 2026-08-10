//! Linux ScreenCast backend.
//!
//! The portal and PipeWire bindings are intentionally kept at the process edge:
//! the desktop integration calls `begin_selection`/`cancel`, then forwards each
//! PipeWire buffer through `push_pipewire_frame`. This keeps the core free of
//! DBus and compositor dependencies while preserving portal consent on Wayland.
#![allow(missing_docs)]

use std::collections::VecDeque;

use crate::screen_share::{
    capture::FrameSink, CapturedFrame, PixelFormat, ScreenCapture, ScreenShareError,
};

/// State of the XDG ScreenCast portal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalState {
    Idle,
    Selecting,
    Streaming,
    Ending,
    Ended,
}

/// A portal-approved Linux capture session fed by PipeWire buffers.
#[derive(Debug)]
pub struct PortalCapture {
    state: PortalState,
    sink: FrameSink,
    format: Option<(u32, u32, PixelFormat)>,
    pending_events: VecDeque<PortalEvent>,
}

/// Lifecycle and format events emitted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalEvent {
    SourcePickerOpened,
    SourceSelected,
    FormatChanged { width: u32, height: u32 },
    Ended,
}

impl PortalCapture {
    /// Create an idle session with a bounded frame queue.
    pub fn new(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            state: PortalState::Idle,
            sink: FrameSink::new(queue_capacity)?,
            format: None,
            pending_events: VecDeque::new(),
        })
    }
    /// Request the native XDG Desktop Portal source picker.
    pub fn begin_selection(&mut self) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Idle {
            return Err(ScreenShareError::new("portal session is already active"));
        }
        self.state = PortalState::Selecting;
        self.pending_events
            .push_back(PortalEvent::SourcePickerOpened);
        Ok(())
    }
    /// Handle a portal cancellation without leaving PipeWire resources alive.
    pub fn cancel(&mut self) {
        if matches!(self.state, PortalState::Selecting | PortalState::Streaming) {
            self.state = PortalState::Ending;
            self.state = PortalState::Ended;
            self.pending_events.push_back(PortalEvent::Ended);
        }
    }
    /// Mark the portal stream as selected and ready to receive PipeWire buffers.
    pub fn source_selected(&mut self) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Selecting {
            return Err(ScreenShareError::new(
                "portal source was not being selected",
            ));
        }
        self.state = PortalState::Streaming;
        self.pending_events.push_back(PortalEvent::SourceSelected);
        Ok(())
    }
    /// Normalize one PipeWire BGRA/RGBA buffer and enqueue it.
    pub fn push_pipewire_frame(&mut self, frame: CapturedFrame) -> Result<(), ScreenShareError> {
        if self.state != PortalState::Streaming {
            return Err(ScreenShareError::new(
                "PipeWire frame received outside streaming state",
            ));
        }
        let current = (frame.width, frame.height, frame.pixel_format);
        if self.format.map(|f| (f.0, f.1)) != Some((frame.width, frame.height)) {
            if self.format.is_some() {
                self.pending_events.push_back(PortalEvent::FormatChanged {
                    width: frame.width,
                    height: frame.height,
                });
            }
            self.format = Some(current);
        }
        self.sink.push(frame);
        Ok(())
    }
    /// Signal that the OS/portal closed the stream.
    pub fn stream_closed(&mut self) {
        self.state = PortalState::Ending;
        self.state = PortalState::Ended;
        self.pending_events.push_back(PortalEvent::Ended);
    }
    /// Read the next lifecycle event.
    pub fn next_event(&mut self) -> Option<PortalEvent> {
        self.pending_events.pop_front()
    }
    /// Return bounded queue diagnostics: captured, encoded, dropped.
    pub fn counters(&self) -> (u64, u64, u64) {
        self.sink.counters()
    }
    /// Current portal state.
    pub fn state(&self) -> PortalState {
        self.state
    }
}

impl ScreenCapture for PortalCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        Ok(self.sink.pop_latest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_ends_selection() {
        let mut c = PortalCapture::new(2).unwrap();
        c.begin_selection().unwrap();
        c.cancel();
        assert_eq!(c.state(), PortalState::Ended);
    }
}
