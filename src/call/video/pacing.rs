//! Deterministic frame pacing and bounded latest-frame buffering.
#![allow(missing_docs)]

use std::collections::VecDeque;

/// Timestamp-based pacer that emits at most the configured target cadence.
#[derive(Debug, Clone, Copy)]
pub struct FramePacer {
    interval_us: u64,
    next_timestamp_us: Option<u64>,
}

impl FramePacer {
    pub fn new(fps: u32) -> Option<Self> {
        (fps > 0).then(|| Self { interval_us: 1_000_000 / fps as u64, next_timestamp_us: None })
    }

    /// Return whether a frame at `timestamp_us` should be emitted.
    pub fn accept(&mut self, timestamp_us: u64) -> bool {
        let Some(next) = self.next_timestamp_us else {
            self.next_timestamp_us = Some(timestamp_us.saturating_add(self.interval_us));
            return true;
        };
        if timestamp_us < next { return false; }
        let elapsed = timestamp_us.saturating_sub(next);
        self.next_timestamp_us = Some(timestamp_us.saturating_add(self.interval_us.saturating_sub(elapsed % self.interval_us)));
        true
    }

    pub fn reset(&mut self) { self.next_timestamp_us = None; }
}

/// A bounded queue with capacity two. When full, the oldest frame is dropped.
#[derive(Debug, Clone)]
pub struct LatestFrameBuffer<T> {
    frames: VecDeque<T>,
}

impl<T> Default for LatestFrameBuffer<T> {
    fn default() -> Self { Self { frames: VecDeque::with_capacity(2) } }
}

impl<T> LatestFrameBuffer<T> {
    pub const CAPACITY: usize = 2;
    pub fn push(&mut self, frame: T) -> Option<T> {
        let dropped = (self.frames.len() == Self::CAPACITY).then(|| self.frames.pop_front()).flatten();
        self.frames.push_back(frame);
        dropped
    }
    pub fn pop_latest(&mut self) -> Option<T> {
        let latest = self.frames.pop_back();
        self.frames.clear();
        latest
    }
    pub fn len(&self) -> usize { self.frames.len() }
    pub fn is_empty(&self) -> bool { self.frames.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pacing_is_deterministic() {
        let mut pacer = FramePacer::new(10).unwrap();
        assert!(pacer.accept(0));
        assert!(!pacer.accept(50_000));
        assert!(pacer.accept(100_000));
        assert!(!pacer.accept(150_000));
        assert!(pacer.accept(200_000));
    }
    #[test]
    fn buffer_is_bounded_and_drops_oldest() {
        let mut queue = LatestFrameBuffer::default();
        assert_eq!(queue.push(1), None);
        assert_eq!(queue.push(2), None);
        assert_eq!(queue.push(3), Some(1));
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop_latest(), Some(3));
    }
}
