//! Device-boundary PCM normalization for voice calls.
//!
//! The call pipeline carries mono `f32` samples at [`INTERNAL_SAMPLE_RATE`].
//! Device formats and rates are converted only at the edge of the pipeline.

use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// The sample rate used by the call pipeline.
pub const INTERNAL_SAMPLE_RATE: u32 = 48_000;
const RESAMPLER_CHUNK: usize = 256;

/// Convert signed 16-bit PCM to normalized floating point samples.
pub fn i16_to_f32(input: &[i16]) -> Vec<f32> {
    input
        .iter()
        .map(|&sample| sample as f32 / 32_768.0)
        .collect()
}

/// Convert unsigned 16-bit PCM to normalized floating point samples.
pub fn u16_to_f32(input: &[u16]) -> Vec<f32> {
    input
        .iter()
        .map(|&sample| (sample as f32 - 32_768.0) / 32_768.0)
        .collect()
}

/// Convert floating point samples to signed 16-bit PCM with clipping.
pub fn f32_to_i16(input: &[f32]) -> Vec<i16> {
    input
        .iter()
        .map(|&sample| {
            (sample.clamp(-1.0, 1.0) * 32_768.0)
                .round()
                .clamp(-32_768.0, 32_767.0) as i16
        })
        .collect()
}

/// Convert floating point samples to unsigned 16-bit PCM with clipping.
pub fn f32_to_u16(input: &[f32]) -> Vec<u16> {
    input
        .iter()
        .map(|&sample| {
            ((sample.clamp(-1.0, 1.0) * 32_768.0) + 32_768.0)
                .round()
                .clamp(0.0, 65_535.0) as u16
        })
        .collect()
}

/// Downmix interleaved device samples to the mono internal representation.
pub fn interleaved_to_mono(input: &[f32], channels: u16) -> Vec<f32> {
    assert!(channels > 0, "audio must have at least one channel");
    input
        .chunks_exact(channels as usize)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

/// A reusable, streaming sample-rate converter.
///
/// Input is retained until a complete resampler chunk is available. This is
/// important for CPAL callbacks: constructing a rubato resampler per callback
/// would allocate and reset its filter history, causing timing spikes and
/// audible discontinuities.
pub struct StatefulResampler {
    input_rate: u32,
    output_rate: u32,
    resampler: Option<Async<f32>>,
    pending: Vec<f32>,
    instance_id: usize,
}

impl std::fmt::Debug for StatefulResampler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StatefulResampler")
            .field("input_rate", &self.input_rate)
            .field("output_rate", &self.output_rate)
            .field("pending_samples", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl StatefulResampler {
    /// Construct a converter from `input_rate` to `output_rate`.
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        assert!(
            input_rate > 0 && output_rate > 0,
            "sample rates must be non-zero"
        );
        let resampler = if input_rate == output_rate {
            None
        } else {
            let parameters = SincInterpolationParameters {
                sinc_len: 128,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            Some(
                Async::new_sinc(
                    output_rate as f64 / input_rate as f64,
                    2.0,
                    &parameters,
                    RESAMPLER_CHUNK,
                    1,
                    FixedAsync::Input,
                )
                .expect("valid audio resampler configuration"),
            )
        };
        Self {
            input_rate,
            output_rate,
            resampler,
            pending: Vec::with_capacity(RESAMPLER_CHUNK),
            instance_id: 0,
        }
    }

    /// Convert the next input segment, preserving filter state between calls.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.resampler.is_none() {
            return input.to_vec();
        }
        self.pending.extend_from_slice(input);
        let complete = self.pending.len() / RESAMPLER_CHUNK * RESAMPLER_CHUNK;
        let mut output = Vec::new();
        if complete == 0 {
            return output;
        }
        for chunk in self.pending[..complete].chunks_exact(RESAMPLER_CHUNK) {
            let input = InterleavedOwned::new_from(chunk.to_vec(), 1, RESAMPLER_CHUNK)
                .expect("chunk has the configured number of frames");
            let converted = self
                .resampler
                .as_mut()
                .expect("resampler exists for non-identity conversion")
                .process(&input, 0, None)
                .expect("resampler input matches configured chunk size");
            output.extend(converted.take_data());
        }
        self.pending.drain(..complete);
        output
    }

    /// Stable identity useful for proving callback processing reuses the same
    /// converter instance. It is intentionally not a memory address.
    pub const fn instance_id(&self) -> usize {
        self.instance_id
    }

    /// Return the configured input and output rates.
    pub const fn rates(&self) -> (u32, u32) {
        (self.input_rate, self.output_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        f32_to_i16, i16_to_f32, interleaved_to_mono, StatefulResampler, INTERNAL_SAMPLE_RATE,
    };

    #[test]
    fn i16_f32_i16_round_trip_is_lossless_within_one_lsb() {
        let input = [-32_768, -16_384, -1, 0, 1, 16_384, 32_767];
        let output = f32_to_i16(&i16_to_f32(&input));
        assert!(input.iter().zip(output).all(|(&a, b)| (a - b).abs() <= 1));
    }

    #[test]
    fn internal_rate_is_48_khz() {
        assert_eq!(INTERNAL_SAMPLE_RATE, 48_000);
    }

    #[test]
    fn stateful_resampler_reuses_one_instance_across_calls() {
        let mut resampler = StatefulResampler::new(INTERNAL_SAMPLE_RATE, 44_100);
        let instance = resampler.instance_id();
        let _ = resampler.process(&[0.0; 1024]);
        let _ = resampler.process(&[0.0; 1024]);
        assert_eq!(resampler.instance_id(), instance);
    }

    #[test]
    fn interleaved_device_audio_is_downmixed_to_mono() {
        assert_eq!(
            interleaved_to_mono(&[1.0, -1.0, 0.25, 0.75], 2),
            vec![0.0, 0.5]
        );
    }

    #[test]
    fn resampling_48k_to_device_rate_changes_length_sensibly() {
        let mut resampler = StatefulResampler::new(INTERNAL_SAMPLE_RATE, 24_000);
        let input = vec![0.0; INTERNAL_SAMPLE_RATE as usize];
        let output = resampler.process(&input);
        assert!((output.len() as i64 - 24_000).abs() < 256);
    }
}
