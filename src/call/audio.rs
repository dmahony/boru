//! Real-time audio capture buffering.
//!
//! The CPAL input callback must only copy samples into this bounded SPSC
//! queue. It must not allocate, wait, lock an async mutex, encode, or perform
//! network I/O. The consumer side is owned by an audio worker and can perform
//! those operations in later pipeline stages.

use rtrb::{Consumer, Producer, RingBuffer};

use super::device::InputCallback;

/// Worker-side Opus encoding for fixed-size voice frames.
pub mod codec;

/// Bounded deadline-driven buffering for received live audio.
pub mod jitter;

/// Opus packet-loss concealment and in-band FEC playout.
pub mod plc;

/// Non-blocking, bounded Opus datagram sending.
#[cfg(feature = "net")]
pub mod send;

/// A bounded producer for CPAL input samples.
///
/// When the queue is full, samples at the end of the callback batch are
/// dropped (drop-newest). This keeps already-captured audio contiguous and,
/// more importantly, never makes the real-time callback wait for the worker.
#[derive(Debug)]
pub struct AudioCaptureProducer {
    producer: Producer<f32>,
    dropped_samples: u64,
}

impl AudioCaptureProducer {
    /// Push as many samples as fit without waiting.
    ///
    /// Returns the number accepted. The remainder is explicitly discarded.
    pub fn push_samples(&mut self, samples: &[f32]) -> usize {
        let (accepted, remainder) = self.producer.push_partial_slice(samples);
        self.dropped_samples += remainder.len() as u64;
        accepted.len()
    }

    /// Number of samples discarded because the queue was full.
    pub const fn dropped_samples(&self) -> u64 {
        self.dropped_samples
    }

    /// Adapt this producer to the CPAL callback boundary.
    pub fn into_callback(mut self) -> InputCallback {
        Box::new(move |samples| {
            let _ = self.push_samples(samples);
        })
    }
}

/// Consumer side of the capture queue, owned by the audio worker.
#[derive(Debug)]
pub struct AudioCaptureConsumer {
    consumer: Consumer<f32>,
}

impl AudioCaptureConsumer {
    /// Copy available samples into `destination` without waiting.
    ///
    /// Returns the number copied. An empty result is normal when the callback
    /// has not produced another batch yet.
    pub fn pop_samples(&mut self, destination: &mut [f32]) -> usize {
        let (popped, _) = self.consumer.pop_partial_slice(destination);
        popped.len()
    }
}

/// Create the callback-side producer and worker-side consumer.
///
/// `capacity` is measured in individual interleaved f32 samples, not CPAL
/// callback batches. A zero-capacity queue is rejected by `rtrb` and should be
/// treated as a configuration error before opening a device.
pub fn new_capture_buffer(capacity: usize) -> (AudioCaptureProducer, AudioCaptureConsumer) {
    let (producer, consumer) = RingBuffer::new(capacity);
    (
        AudioCaptureProducer {
            producer,
            dropped_samples: 0,
        },
        AudioCaptureConsumer { consumer },
    )
}

#[cfg(test)]
mod tests {
    use super::new_capture_buffer;

    #[test]
    fn overload_drops_newest_and_respects_bound() {
        let (mut producer, mut consumer) = new_capture_buffer(4);
        assert_eq!(producer.push_samples(&[1.0, 2.0, 3.0, 4.0, 5.0]), 4);
        assert_eq!(producer.dropped_samples(), 1);

        let mut output = [0.0; 8];
        assert_eq!(consumer.pop_samples(&mut output), 4);
        assert_eq!(&output[..4], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(consumer.pop_samples(&mut output), 0);
    }

    #[test]
    fn callback_is_synchronous_and_non_blocking_when_full() {
        let (producer, mut consumer) = new_capture_buffer(2);
        let mut callback = producer.into_callback();

        callback(&[10.0, 20.0]);
        callback(&[30.0, 40.0]);

        let mut output = [0.0; 4];
        assert_eq!(consumer.pop_samples(&mut output), 2);
        assert_eq!(&output[..2], &[10.0, 20.0]);
    }
}
