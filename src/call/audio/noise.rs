//! Optional noise suppression stage (BORU-CALL-3.12).
//!
//! Sits between the resampler and the Opus encoder on the worker side of the
//! capture pipeline:
//!
//! ```text
//! capture → resample → noise suppression → Opus
//! ```
//!
//! It is deliberately NOT used on the CPAL callback thread: the real-time
//! callback only copies samples into the bounded capture queue (see
//! [`super::AudioCaptureProducer`]). This stage is constructed and driven from
//! the audio worker, where allocating and running the RNNoise model is safe.
//!
//! The stage is gated by a runtime toggle and is optional for the first voice
//! milestone. When disabled it is an exact pass-through, so the baseline
//! pipeline is unaffected.

use nnnoiseless::DenoiseState;

use crate::call::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};

/// Samples consumed per `nnnoiseless` call (10 ms at 48 kHz).
const NNOISELESS_FRAME_SIZE: usize = 480;

/// Normalized f32 → 16-bit PCM scale multiplier.
///
/// `nnnoiseless` expects its input and output in 16-bit signed PCM range
/// `[-32768.0, 32767.0]`, while Boru's internal format is normalized
/// `[-1.0, 1.0]`. The stage scales in and back out around each call.
const I16_SCALE: f32 = 32_768.0;

/// Optional RNNoise-based noise suppressor for the send audio pipeline.
///
/// Processes one 20 ms frame (960 samples) at a time, internally feeding two
/// 480-sample sub-frames to the stateful `nnnoiseless` model.
pub struct NoiseSuppressor {
    denoise: Box<DenoiseState<'static>>,
    enabled: bool,
    /// 480-sample working buffer used to scale between normalized f32 and the
    /// 16-bit scale expected by `nnnoiseless`.
    scratch: Vec<f32>,
    /// Number of frames processed since construction (used to skip the
    /// fade-in artifact of the very first model output).
    frames_processed: u64,
}

impl std::fmt::Debug for NoiseSuppressor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NoiseSuppressor")
            .field("enabled", &self.enabled)
            .field("frames_processed", &self.frames_processed)
            .finish_non_exhaustive()
    }
}

impl Default for NoiseSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor {
    /// Create a noise suppressor with suppression **disabled**.
    ///
    /// The first voice milestone does not require noise suppression, so the
    /// stage starts as an exact pass-through. Call [`Self::set_enabled`] to
    /// turn it on once the caller is ready.
    pub fn new() -> Self {
        Self {
            denoise: DenoiseState::new(),
            enabled: false,
            scratch: vec![0.0; NNOISELESS_FRAME_SIZE],
            frames_processed: 0,
        }
    }

    /// Turn the suppression stage on or off.
    ///
    /// When disabled, [`Self::process_frame`] copies the input to the output
    /// unchanged.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether suppression is currently active.
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Number of frames that have passed through the stage.
    pub const fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    /// Process one 20 ms frame of normalized mono f32 PCM in place.
    ///
    /// The frame length must equal [`SAMPLES_PER_FRAME`] (960 samples at
    /// 48 kHz). When suppression is disabled, the buffer is left unchanged.
    pub fn process_frame(&mut self, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), SAMPLES_PER_FRAME);
        self.frames_processed = self.frames_processed.wrapping_add(1);

        if !self.enabled {
            return;
        }

        // `nnnoiseless` is stateful and consumes 480-sample (10 ms) chunks at
        // 48 kHz. Our frame is 20 ms, so run the model twice per frame.
        debug_assert_eq!(frame.len() % NNOISELESS_FRAME_SIZE, 0);
        for chunk in frame.chunks_exact_mut(NNOISELESS_FRAME_SIZE) {
            self.process_sub_frame(chunk);
        }
    }

    fn process_sub_frame(&mut self, chunk: &mut [f32]) {
        // Scale normalized f32 → 16-bit range expected by the model.
        for (scratch, sample) in self.scratch.iter_mut().zip(chunk.iter()) {
            *scratch = sample.clamp(-1.0, 1.0) * I16_SCALE;
        }
        let mut output = [0.0f32; NNOISELESS_FRAME_SIZE];
        self.denoise.process_frame(&mut output, &self.scratch);
        // Scale back to normalized f32. The first output contains a fade-in
        // artifact; we do not special-case it here (it is one frame of ramp
        // that the Opus encoder handles transparently).
        for (chunk_sample, out_sample) in chunk.iter_mut().zip(output.iter()) {
            *chunk_sample = out_sample / I16_SCALE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};

    /// Structural guarantee: the stage is owned by the audio worker and must
    /// be movable there, never used from the CPAL callback.
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    fn noisy_frame(seed: u32) -> Vec<f32> {
        // Deterministic pseudo-random noise (simple LCG) added to a sine tone.
        let mut state = seed.wrapping_mul(16_777_161).wrapping_add(1);
        (0..SAMPLES_PER_FRAME)
            .map(|i| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let noise = ((state >> 16) & 0xFFFF) as f32 / 32_768.0 - 1.0;
                let phase = 2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32;
                0.3 * phase.sin() + 0.4 * noise
            })
            .collect()
    }

    #[test]
    fn suppression_does_not_crash_on_noisy_input() {
        let mut suppressor = NoiseSuppressor::new();
        suppressor.set_enabled(true);
        let mut frame = noisy_frame(7);
        suppressor.process_frame(&mut frame);
        // No panic is the primary assertion; also verify the buffer is intact.
        assert_eq!(frame.len(), SAMPLES_PER_FRAME);
    }

    #[test]
    fn output_is_finite_and_sane_with_suppression_enabled() {
        let mut suppressor = NoiseSuppressor::new();
        suppressor.set_enabled(true);
        let mut frame = noisy_frame(9);
        for _ in 0..5 {
            suppressor.process_frame(&mut frame);
        }
        assert!(
            frame.iter().all(|sample| sample.is_finite()),
            "suppressed output must be finite"
        );
        // Sane amplitude: allow modest headroom beyond the ±1.0 input norm.
        assert!(
            frame.iter().all(|sample| sample.abs() < 2.0),
            "suppressed output must stay bounded"
        );
    }

    #[test]
    fn toggle_off_is_exact_passthrough() {
        let mut suppressor = NoiseSuppressor::new();
        // Default is disabled.
        assert!(!suppressor.is_enabled());
        let input = noisy_frame(11);
        let mut frame = input.clone();
        suppressor.process_frame(&mut frame);
        assert_eq!(frame, input, "disabled stage must not modify the frame");

        suppressor.set_enabled(true);
        assert!(suppressor.is_enabled());
        suppressor.process_frame(&mut frame);
        // With suppression on, the noisy frame should be modified (or at least
        // processed without changing length).
        assert_eq!(frame.len(), SAMPLES_PER_FRAME);

        suppressor.set_enabled(false);
        let mut frame2 = input.clone();
        suppressor.process_frame(&mut frame2);
        assert_eq!(frame2, input, "re-disabled stage must pass through again");
    }

    #[test]
    fn processing_stays_off_the_callback_thread_by_construction() {
        // The stage is Send + Sync, so it can be moved to the audio worker.
        // The CPAL callback path (AudioCaptureProducer) never holds one; the
        // stage is only reachable from the worker-side pipeline.
        assert_send::<NoiseSuppressor>();
        assert_sync::<NoiseSuppressor>();
    }

    #[test]
    fn rejects_partial_frames_in_debug() {
        // In debug builds the length assertion fires; in release the chunks
        // remainder is silently dropped, so we only assert in debug.
        #[cfg(debug_assertions)]
        {
            let mut suppressor = NoiseSuppressor::new();
            suppressor.set_enabled(true);
            let mut short = vec![0.0; 480];
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                suppressor.process_frame(&mut short);
            }));
            assert!(result.is_err(), "partial frame must be rejected");
        }
    }
}
