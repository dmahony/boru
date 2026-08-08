//! Opus encoding for the worker-side live-call audio pipeline.
//!
//! The encoder is deliberately separate from the CPAL callback. Construct and
//! use it from the audio worker, never from the real-time device callback.

use anyhow::{ensure, Result};
use opus::{Application, Bitrate, Channels, Encoder, FrameSize};

use super::super::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};

/// Bitrate used by the initial voice-call profile.
pub const DEFAULT_BITRATE: i32 = 32_000;
/// Expected packet loss used to enable Opus's in-band FEC tuning.
pub const DEFAULT_PACKET_LOSS_PERCENT: i32 = 5;
/// Encoder complexity: a middle-of-the-road CPU/quality choice (0..=10).
pub const DEFAULT_COMPLEXITY: i32 = 5;
/// Maximum encoded packet size accepted from libopus.
const MAX_PACKET_SIZE: usize = 1_275;

/// Stateful Opus encoder for 48 kHz mono, 20 ms voice frames.
///
/// Opus maintains prediction state between calls, so one instance must be
/// reused for the duration of a call. The input must contain exactly one
/// [`SAMPLES_PER_FRAME`]-sample frame.
pub struct OpusEncoder {
    encoder: Encoder,
}

impl std::fmt::Debug for OpusEncoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpusEncoder")
            .finish_non_exhaustive()
    }
}

impl OpusEncoder {
    /// Create an encoder with the fixed initial VOIP profile.
    pub fn new() -> Result<Self> {
        let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::Bits(DEFAULT_BITRATE))?;
        encoder.set_vbr(true)?;
        encoder.set_force_channels(Some(Channels::Mono))?;
        encoder.set_expert_frame_duration(FrameSize::Ms20)?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(DEFAULT_PACKET_LOSS_PERCENT)?;
        encoder.set_dtx(true)?;
        encoder.set_complexity(DEFAULT_COMPLEXITY)?;
        Ok(Self { encoder })
    }

    /// Encode one 20 ms frame of normalized mono f32 PCM.
    ///
    /// Returns `None` only when libopus emits an empty DTX result. A DTX
    /// comfort-noise/SID packet remains `Some`, because it is meaningful to
    /// the receiver and must be transmitted.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Option<Vec<u8>>> {
        ensure!(
            pcm.len() == SAMPLES_PER_FRAME,
            "Opus expects exactly {SAMPLES_PER_FRAME} samples, got {}",
            pcm.len()
        );
        let packet = self.encoder.encode_vec_float(pcm, MAX_PACKET_SIZE)?;
        Ok((!packet.is_empty()).then_some(packet))
    }

    #[cfg(test)]
    fn inner_mut(&mut self) -> &mut Encoder {
        &mut self.encoder
    }
}

#[cfg(test)]
mod tests {
    use super::{OpusEncoder, DEFAULT_BITRATE, DEFAULT_COMPLEXITY, DEFAULT_PACKET_LOSS_PERCENT};
    use crate::call::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};
    use opus::{Application, Bitrate, Channels, FrameSize};

    #[test]
    fn encodes_a_synthetic_voice_frame() {
        let mut encoder = OpusEncoder::new().expect("libopus encoder should initialize");
        let pcm: Vec<f32> = (0..SAMPLES_PER_FRAME)
            .map(|sample| (sample as f32 * 0.05).sin() * 0.2)
            .collect();
        let packet = encoder.encode(&pcm).expect("frame should encode");
        assert!(packet.is_some(), "a non-silent frame must produce a packet");
        assert!(packet.as_ref().is_some_and(|packet| !packet.is_empty()));
    }

    #[test]
    fn encoder_is_reusable_across_frames() {
        let mut encoder = OpusEncoder::new().expect("libopus encoder should initialize");
        let first = vec![0.1; SAMPLES_PER_FRAME];
        let second = vec![-0.1; SAMPLES_PER_FRAME];
        assert!(encoder.encode(&first).expect("first frame").is_some());
        assert!(encoder.encode(&second).expect("second frame").is_some());
    }

    #[test]
    fn voip_defaults_are_applied() {
        let mut encoder = OpusEncoder::new().expect("libopus encoder should initialize");
        let inner = encoder.inner_mut();
        assert_eq!(inner.get_application().unwrap(), Application::Voip);
        assert_eq!(inner.get_force_channels().unwrap(), Some(Channels::Mono));
        assert_eq!(inner.get_bitrate().unwrap(), Bitrate::Bits(DEFAULT_BITRATE));
        assert!(inner.get_vbr().unwrap());
        assert_eq!(inner.get_expert_frame_duration().unwrap(), FrameSize::Ms20);
        assert!(inner.get_inband_fec().unwrap());
        assert!(inner.get_dtx().unwrap());
        assert_eq!(
            inner.get_packet_loss_perc().unwrap(),
            DEFAULT_PACKET_LOSS_PERCENT
        );
        assert_eq!(inner.get_complexity().unwrap(), DEFAULT_COMPLEXITY);
        assert_eq!(SAMPLE_RATE, 48_000);
    }

    #[test]
    fn rejects_partial_frames() {
        let mut encoder = OpusEncoder::new().expect("libopus encoder should initialize");
        let error = encoder.encode(&[0.0; SAMPLES_PER_FRAME - 1]).unwrap_err();
        assert!(error.to_string().contains("exactly 960 samples"));
    }

    #[test]
    fn silence_is_valid_input() {
        let mut encoder = OpusEncoder::new().expect("libopus encoder should initialize");
        let result = encoder.encode(&[0.0; SAMPLES_PER_FRAME]).unwrap();
        assert!(result.is_none() || result.as_ref().is_some_and(|packet| !packet.is_empty()));
    }
}
