//! Non-blocking receive and playback pipeline for live Opus audio.
//!
//! The worker side parses media datagrams, feeds the bounded jitter buffer,
//! decodes due packets, converts them to the selected device rate, and pushes
//! interleaved samples into a bounded SPSC ring. The CPAL callback only drains
//! that ring and writes silence for samples that are not available.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use rtrb::{Consumer, Producer, RingBuffer};

use super::jitter::{AudioJitterBuffer, BufferedAudioPacket};
use super::plc::OpusPlayoutDecoder;
use crate::call::device::OutputCallback;
use crate::call::format::StatefulResampler;
use crate::call::frame::SAMPLE_RATE;
use crate::call::media::{MediaDatagram, MediaKind};

/// A bounded SPSC playback queue split between the audio worker and CPAL.
#[derive(Debug)]
pub struct PlaybackRing;

impl PlaybackRing {
    /// Create a ring measured in interleaved device samples.
    pub fn new(capacity: usize) -> (PlaybackProducer, PlaybackConsumer) {
        Self::new_with_control(capacity, Arc::new(AudioPlaybackControl::default()))
    }

    /// Create a playback ring controlled by a shared local deafen gate.
    pub fn new_with_control(
        capacity: usize,
        control: Arc<AudioPlaybackControl>,
    ) -> (PlaybackProducer, PlaybackConsumer) {
        let (producer, consumer) = RingBuffer::new(capacity);
        (
            PlaybackProducer { producer },
            PlaybackConsumer {
                consumer,
                underruns: Arc::new(AtomicU64::new(0)),
                control,
            },
        )
    }
}

/// Local playback controls. Deafen is deliberately not signalled to peers.
#[derive(Debug, Default)]
pub struct AudioPlaybackControl {
    deafened: AtomicBool,
}

impl AudioPlaybackControl {
    /// Enable or disable local playback suppression.
    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Release);
    }

    /// Return the current local playback suppression state.
    pub fn is_deafened(&self) -> bool {
        self.deafened.load(Ordering::Acquire)
    }
}

/// Worker-side producer for decoded, device-rate samples.
#[derive(Debug)]
pub struct PlaybackProducer {
    producer: Producer<f32>,
}

impl PlaybackProducer {
    /// Push as many samples as fit, dropping newest samples when full.
    pub fn push_samples(&mut self, samples: &[f32]) -> usize {
        let (accepted, _) = self.producer.push_partial_slice(samples);
        accepted.len()
    }

    /// Number of samples currently available to the output callback.
    pub fn slots(&self) -> usize {
        self.producer.slots()
    }
}

/// CPAL-side consumer. All methods are non-blocking.
#[derive(Debug)]
pub struct PlaybackConsumer {
    consumer: Consumer<f32>,
    underruns: Arc<AtomicU64>,
    control: Arc<AudioPlaybackControl>,
}

impl PlaybackConsumer {
    /// Number of callbacks that had to insert silence.
    pub fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Fill an interleaved CPAL output buffer without waiting or allocating.
    pub fn fill_output(&mut self, output: &mut [f32]) {
        if self.control.is_deafened() {
            output.fill(0.0);
            return;
        }
        let (available, remainder) = self.consumer.pop_partial_slice(output);
        for sample in remainder {
            *sample = 0.0;
        }
        if available.len() != output.len() {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Adapt this consumer to the CPAL output callback boundary.
    pub fn into_callback(mut self) -> OutputCallback {
        Box::new(move |output| self.fill_output(output))
    }

    /// Access the shared local playback control.
    pub fn control(&self) -> Arc<AudioPlaybackControl> {
        Arc::clone(&self.control)
    }
}

/// Stateful worker-side receive pipeline for one call and audio track.
#[derive(Debug)]
pub struct AudioReceiver {
    call_id: crate::call::CallId,
    track_id: u32,
    jitter: AudioJitterBuffer,
    decoder: OpusPlayoutDecoder,
    resampler: StatefulResampler,
    channels: u16,
    playback: PlaybackProducer,
}

impl AudioReceiver {
    /// Construct a receiver. `playback_capacity` is interleaved device samples.
    pub fn new(
        call_id: crate::call::CallId,
        track_id: u32,
        output_rate: u32,
        channels: u16,
        playback_capacity: usize,
    ) -> Result<(Self, PlaybackConsumer)> {
        Self::new_with_control(
            call_id,
            track_id,
            output_rate,
            channels,
            playback_capacity,
            Arc::new(AudioPlaybackControl::default()),
        )
    }

    /// Construct a receiver with a control shared by the active-call UI.
    pub fn new_with_control(
        call_id: crate::call::CallId,
        track_id: u32,
        output_rate: u32,
        channels: u16,
        playback_capacity: usize,
        control: Arc<AudioPlaybackControl>,
    ) -> Result<(Self, PlaybackConsumer)> {
        if track_id == 0 {
            return Err(anyhow!("audio track id must be non-zero"));
        }
        if channels == 0 {
            return Err(anyhow!("audio output must have at least one channel"));
        }
        let (playback, consumer) = PlaybackRing::new_with_control(playback_capacity, control);
        Ok((
            Self {
                call_id,
                track_id,
                jitter: AudioJitterBuffer::default(),
                decoder: OpusPlayoutDecoder::new()?,
                resampler: StatefulResampler::new(SAMPLE_RATE, output_rate),
                channels,
                playback,
            },
            consumer,
        ))
    }

    /// Parse and enqueue one complete audio media datagram without waiting.
    /// Returns false for a valid packet that the jitter buffer rejected.
    pub fn receive_datagram(&mut self, datagram: &[u8], arrival: Instant) -> Result<bool> {
        let packet = MediaDatagram::parse(datagram).context("parse received audio datagram")?;
        if packet.kind != MediaKind::Audio {
            return Err(anyhow!("received non-audio media datagram"));
        }
        if packet.call_id != self.call_id || packet.track_id != self.track_id {
            return Ok(false);
        }
        if packet.fragment_count != 1 || packet.fragment_index != 0 {
            return Ok(false);
        }
        Ok(self.jitter.push(BufferedAudioPacket {
            call_id: packet.call_id,
            sequence: packet.sequence,
            timestamp: packet.timestamp,
            arrival,
            payload: packet.payload,
        }))
    }

    /// Decode every frame currently due and enqueue converted playback samples.
    /// This never waits; a full playback ring drops newest samples.
    pub fn process_due(&mut self, now: Instant) -> Result<usize> {
        let mut decoded = 0;
        while let Some(frame) = self.decoder.decode_due(&mut self.jitter, now)? {
            let mono = self.resampler.process(&frame.samples);
            if mono.is_empty() {
                continue;
            }
            let mut interleaved = Vec::with_capacity(mono.len() * self.channels as usize);
            for sample in mono {
                interleaved.extend(std::iter::repeat_n(sample, self.channels as usize));
            }
            self.playback.push_samples(&interleaved);
            decoded += 1;
        }
        Ok(decoded)
    }

    /// Access the jitter buffer for diagnostics and scheduling.
    pub fn jitter(&self) -> &AudioJitterBuffer {
        &self.jitter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::audio::codec::OpusEncoder;
    use crate::call::frame::SAMPLES_PER_FRAME;
    use crate::call::media::MediaDatagram;
    use crate::call::CallId;

    fn packet(call_id: CallId, sequence: u32, payload: Vec<u8>) -> Vec<u8> {
        MediaDatagram {
            kind: MediaKind::Audio,
            flags: 0,
            call_id,
            track_id: 1,
            sequence,
            timestamp: sequence * SAMPLES_PER_FRAME as u32,
            fragment_index: 0,
            fragment_count: 1,
            payload,
        }
        .encode()
    }

    #[test]
    fn callback_fills_silence_and_counts_underrun() {
        let (mut producer, mut consumer) = PlaybackRing::new(4);
        producer.push_samples(&[0.25, -0.5]);
        let mut output = [9.0; 4];
        consumer.fill_output(&mut output);
        assert_eq!(output, [0.25, -0.5, 0.0, 0.0]);
        assert_eq!(consumer.underrun_count(), 1);
    }

    #[test]
    fn playback_ring_is_bounded_and_does_not_wait() {
        let (mut producer, mut consumer) = PlaybackRing::new(2);
        assert_eq!(producer.push_samples(&[1.0, 2.0, 3.0]), 2);
        let mut output = [0.0; 2];
        consumer.fill_output(&mut output);
        assert_eq!(output, [1.0, 2.0]);
        assert_eq!(consumer.underrun_count(), 0);
    }

    #[test]
    fn deafen_gate_outputs_silence_without_consuming_playback() {
        let control = Arc::new(AudioPlaybackControl::default());
        let (mut producer, mut consumer) = PlaybackRing::new_with_control(2, Arc::clone(&control));
        producer.push_samples(&[0.25, -0.5]);
        control.set_deafened(true);
        let mut output = [9.0; 2];
        consumer.fill_output(&mut output);
        assert_eq!(output, [0.0, 0.0]);
        control.set_deafened(false);
        consumer.fill_output(&mut output);
        assert_eq!(output, [0.25, -0.5]);
    }

    #[test]
    fn datagram_decode_to_playback_pcm() {
        let call = CallId::from_bytes([7; 16]);
        let mut encoder = OpusEncoder::new().unwrap();
        let payload = encoder
            .encode(&vec![0.2; SAMPLES_PER_FRAME])
            .unwrap()
            .unwrap();
        let start = Instant::now();
        let (mut receiver, mut output) =
            AudioReceiver::new(call, 1, SAMPLE_RATE, 1, SAMPLES_PER_FRAME * 2).unwrap();
        assert!(receiver
            .receive_datagram(&packet(call, 0, payload), start)
            .unwrap());
        assert_eq!(receiver.process_due(start).unwrap(), 0);
        assert_eq!(
            receiver
                .process_due(start + super::super::jitter::DEFAULT_JITTER_DELAY)
                .unwrap(),
            1
        );
        let mut samples = [0.0; SAMPLES_PER_FRAME];
        output.fill_output(&mut samples);
        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
    }
}
