//! Windows Graphics Capture backend boundary.
//!
//! The WinRT adapter owns picker/COM objects and forwards acquired surfaces as
//! GPU handles (or copies BGRA buffers when CPU encoding is selected). Keeping
//! Keeping the adapter behind this module makes shutdown and format changes explicit.
#![allow(missing_docs)]

use std::collections::VecDeque;

use crate::screen_share::{capture::FrameSink, CapturedFrame, ScreenCapture, ScreenShareError};

/// Lifecycle of a Windows Graphics Capture session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCaptureState {
    Idle,
    Selecting,
    Streaming,
    Ending,
    Ended,
}

/// Format change or source lifecycle notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCaptureEvent {
    PickerOpened,
    SourceSelected,
    FormatChanged { width: u32, height: u32 },
    SourceMinimized,
    Ended,
}

/// Capture session fed by the WinRT frame-pool callback.
#[derive(Debug)]
pub struct GraphicsCapture {
    state: GraphicsCaptureState,
    sink: FrameSink,
    format: Option<(u32, u32)>,
    events: VecDeque<GraphicsCaptureEvent>,
}

impl GraphicsCapture {
    /// Create a bounded capture session.
    pub fn new(queue_capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            state: GraphicsCaptureState::Idle,
            sink: FrameSink::new(queue_capacity)?,
            format: None,
            events: VecDeque::new(),
        })
    }
    /// Open the system picker for a display or window.
    pub fn begin_selection(&mut self) -> Result<(), ScreenShareError> {
        if self.state != GraphicsCaptureState::Idle {
            return Err(ScreenShareError::new(
                "graphics capture session is already active",
            ));
        }
        self.state = GraphicsCaptureState::Selecting;
        self.events.push_back(GraphicsCaptureEvent::PickerOpened);
        Ok(())
    }
    /// Complete picker selection.
    pub fn source_selected(&mut self) -> Result<(), ScreenShareError> {
        if self.state != GraphicsCaptureState::Selecting {
            return Err(ScreenShareError::new(
                "graphics source was not being selected",
            ));
        }
        self.state = GraphicsCaptureState::Streaming;
        self.events.push_back(GraphicsCaptureEvent::SourceSelected);
        Ok(())
    }
    /// Accept a frame-pool surface without forcing a CPU copy.
    pub fn push_surface(&mut self, frame: CapturedFrame) -> Result<(), ScreenShareError> {
        if self.state != GraphicsCaptureState::Streaming {
            return Err(ScreenShareError::new(
                "graphics frame received outside streaming state",
            ));
        }
        if self.format != Some((frame.width, frame.height)) {
            if self.format.is_some() {
                self.events.push_back(GraphicsCaptureEvent::FormatChanged {
                    width: frame.width,
                    height: frame.height,
                });
            }
            self.format = Some((frame.width, frame.height));
        }
        self.sink.push(frame);
        Ok(())
    }
    /// Notify the backend that the source became minimized.
    pub fn source_minimized(&mut self) {
        self.events.push_back(GraphicsCaptureEvent::SourceMinimized);
    }
    /// Release the frame pool and all source resources.
    pub fn close(&mut self) {
        self.state = GraphicsCaptureState::Ending;
        self.state = GraphicsCaptureState::Ended;
        self.events.push_back(GraphicsCaptureEvent::Ended);
    }
    /// Read the next lifecycle event.
    pub fn next_event(&mut self) -> Option<GraphicsCaptureEvent> {
        self.events.pop_front()
    }
    /// Return bounded queue diagnostics: captured, encoded, dropped.
    pub fn counters(&self) -> (u64, u64, u64) {
        self.sink.counters()
    }
    /// Current state.
    pub fn state(&self) -> GraphicsCaptureState {
        self.state
    }
}

impl ScreenCapture for GraphicsCapture {
    fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
        Ok(self.sink.pop_latest())
    }
}
