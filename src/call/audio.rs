//! Real-time audio capture buffering.
//!
//! The CPAL input callback must only copy samples into this bounded SPSC
//! queue. It must not allocate, wait, lock an async mutex, encode, or perform
//! network I/O. The consumer side is owned by an audio worker and can perform
//! those operations in later pipeline stages.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

use rtrb::{Consumer, Producer, RingBuffer};

use super::device::InputCallback;

/// Lightweight peak/RMS meter updated by the audio worker.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioLevel {
    /// Peak absolute amplitude in the measured batch.
    pub peak: f32,
    /// Root-mean-square amplitude in the measured batch.
    pub rms: f32,
}

impl AudioLevel {
    /// Measure a PCM batch and clamp invalid samples to a safe display range.
    pub fn from_samples(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut peak: f32 = 0.0;
        let mut energy: f32 = 0.0;
        for &sample in samples {
            let sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            peak = peak.max(sample.abs());
            energy += sample * sample;
        }
        Self {
            peak,
            rms: (energy / samples.len() as f32).sqrt(),
        }
    }
}

/// Lock-free level meter shared between a device callback and the UI.
#[derive(Debug, Default)]
pub struct AudioLevelMeter {
    peak: AtomicU32,
    rms: AtomicU32,
}

impl AudioLevelMeter {
    /// Update the most recent bounded PCM batch.
    pub fn update(&self, level: AudioLevel) {
        self.peak.store(level.peak.to_bits(), Ordering::Relaxed);
        self.rms.store(level.rms.to_bits(), Ordering::Relaxed);
    }

    /// Read the most recent level without blocking the audio callback.
    pub fn level(&self) -> AudioLevel {
        AudioLevel {
            peak: f32::from_bits(self.peak.load(Ordering::Relaxed)),
            rms: f32::from_bits(self.rms.load(Ordering::Relaxed)),
        }
    }
}

/// Worker-side Opus encoding for fixed-size voice frames.
pub mod codec;

/// Bounded deadline-driven buffering for received live audio.
pub mod jitter;

/// Optional RNNoise-based noise suppression for the send pipeline.
pub mod noise;

/// Opus packet-loss concealment and in-band FEC playout.
pub mod plc;

/// Received audio decode, resampling, playback-ring, and output callback path.
pub mod receive;

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
    muted: Arc<AtomicBool>,
    meter: Arc<AudioLevelMeter>,
}

impl AudioCaptureProducer {
    /// Push as many samples as fit without waiting.
    ///
    /// Returns the number accepted. The remainder is explicitly discarded.
    pub fn push_samples(&mut self, samples: &[f32]) -> usize {
        self.meter.update(AudioLevel::from_samples(samples));
        if self.is_muted() {
            return 0;
        }
        let (accepted, remainder) = self.producer.push_partial_slice(samples);
        self.dropped_samples += remainder.len() as u64;
        accepted.len()
    }

    /// Measure a callback batch before it is queued for encoding.
    pub fn level(samples: &[f32]) -> AudioLevel {
        AudioLevel::from_samples(samples)
    }

    /// Return the live callback meter associated with this producer.
    pub fn meter(&self) -> Arc<AudioLevelMeter> {
        Arc::clone(&self.meter)
    }

    /// Number of samples discarded because the queue was full.
    pub const fn dropped_samples(&self) -> u64 {
        self.dropped_samples
    }

    /// Suppress samples at the capture boundary without stopping the device.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    /// Return the current capture mute state.
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
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
    let (producer, consumer, _) = new_capture_buffer_with_meter(capacity);
    (producer, consumer)
}

/// Create a capture queue and expose its lock-free live level meter.
pub fn new_capture_buffer_with_meter(
    capacity: usize,
) -> (
    AudioCaptureProducer,
    AudioCaptureConsumer,
    Arc<AudioLevelMeter>,
) {
    let (producer, consumer) = RingBuffer::new(capacity);
    let meter = Arc::new(AudioLevelMeter::default());
    (
        AudioCaptureProducer {
            producer,
            dropped_samples: 0,
            muted: Arc::new(AtomicBool::new(false)),
            meter: Arc::clone(&meter),
        },
        AudioCaptureConsumer { consumer },
        meter,
    )
}

#[cfg(test)]
mod tests {
    use super::{new_capture_buffer, new_capture_buffer_with_meter, AudioLevel};

    #[test]
    fn level_meter_reports_peak_and_rms_and_handles_bad_samples() {
        let level = AudioLevel::from_samples(&[-1.0, 1.0, f32::NAN, 0.0]);
        assert_eq!(level.peak, 1.0);
        assert!((level.rms - 0.707).abs() < 0.01);
        assert_eq!(AudioLevel::from_samples(&[]), AudioLevel::default());
    }

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

    #[test]
    fn muted_capture_drops_samples_and_unmute_resumes() {
        let (mut producer, mut consumer) = new_capture_buffer(4);
        producer.set_muted(true);
        assert_eq!(producer.push_samples(&[1.0, 2.0]), 0);
        let mut output = [0.0; 4];
        assert_eq!(consumer.pop_samples(&mut output), 0);

        producer.set_muted(false);
        assert_eq!(producer.push_samples(&[3.0, 4.0]), 2);
        assert_eq!(consumer.pop_samples(&mut output), 2);
        assert_eq!(&output[..2], &[3.0, 4.0]);
    }

    #[test]
    fn live_meter_is_updated_by_the_capture_callback() {
        let (producer, _, meter) = new_capture_buffer_with_meter(8);
        let mut callback = producer.into_callback();
        callback(&[-0.5, 0.5]);
        let level = meter.level();
        assert_eq!(level.peak, 0.5);
        assert!((level.rms - 0.5).abs() < f32::EPSILON);
    }
}
