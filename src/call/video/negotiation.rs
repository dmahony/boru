//! Deterministic v2 media negotiation and codec-init recovery.
#![allow(missing_docs)]

use super::super::bounds::{MAX_VIDEO_FPS, MAX_VIDEO_HEIGHT, MAX_VIDEO_WIDTH};
use super::super::wire::{NegotiatedVideoV2, VideoCodec, VideoCodecCapability};

pub const AUDIO_RESERVE_BPS: u32 = 64_000;
pub const TOTAL_MEDIA_BUDGET_BPS: u32 = 864_000;
pub const ADVERTISED_SPATIAL_LAYERS: u8 = 1;
pub const ADVERTISED_TEMPORAL_LAYERS: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitRecovery { RecoveredH264, RequiresNewTrack, NoFallback }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Av1RolloutMode { #[default] Auto, H264Only, Av1Preferred }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecFailureReason { OverBudget, DecodeFailure, ContextFailure, EncoderUnavailable }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecFailure { pub track_id: u32, pub reason: CodecFailureReason }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FallbackDiagnostics {
    pub av1_selected: u64,
    pub av1_failures: u64,
    pub h264_fallbacks: u64,
    pub last_failure: Option<CodecFailureReason>,
}
impl FallbackDiagnostics {
    pub fn record_av1_selected(&mut self) { self.av1_selected = self.av1_selected.saturating_add(1); }
    pub fn record_failure(&mut self, reason: CodecFailureReason) { self.av1_failures = self.av1_failures.saturating_add(1); self.last_failure = Some(reason); }
    pub fn record_h264_fallback(&mut self) { self.h264_fallbacks = self.h264_fallbacks.saturating_add(1); }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackCodecState { pub track_id: u32, pub codec: VideoCodec }

pub fn fallback_codec(state: TrackCodecState, fallback_available: bool) -> Option<VideoCodec> {
    (state.codec == VideoCodec::Av1 && fallback_available).then_some(VideoCodec::H264)
}

impl TrackCodecState {
    pub fn recover_from_init_failure(&self, fallback_available: bool) -> InitRecovery {
        if self.codec == VideoCodec::Av1 && fallback_available { InitRecovery::RequiresNewTrack }
        else if self.codec == VideoCodec::Av1 { InitRecovery::NoFallback }
        else { InitRecovery::NoFallback }
    }
}

pub fn negotiate_video(local_encode: &[VideoCodecCapability], local_decode: &[VideoCodecCapability], remote_encode: &[VideoCodecCapability], remote_decode: &[VideoCodecCapability], track_id: u32) -> Option<NegotiatedVideoV2> {
    negotiate_video_with_mode(local_encode, local_decode, remote_encode, remote_decode, track_id, Av1RolloutMode::Av1Preferred)
}

pub fn negotiate_video_with_mode(local_encode: &[VideoCodecCapability], local_decode: &[VideoCodecCapability], remote_encode: &[VideoCodecCapability], remote_decode: &[VideoCodecCapability], track_id: u32, mode: Av1RolloutMode) -> Option<NegotiatedVideoV2> {
    if track_id == 0 { return None; }
    let preference = match mode { Av1RolloutMode::Auto | Av1RolloutMode::H264Only => [VideoCodec::H264, VideoCodec::Av1], Av1RolloutMode::Av1Preferred => [VideoCodec::Av1, VideoCodec::H264] };
    preference.into_iter().find_map(|codec| {
        if mode == Av1RolloutMode::H264Only && codec == VideoCodec::Av1 { return None; }
        let caps = [local_encode, local_decode, remote_encode, remote_decode].iter().filter_map(|list| list.iter().find(|c| c.codec == codec)).collect::<Vec<_>>();
        if caps.len() != 4 { return None; }
        let width = caps.iter().map(|c| c.max_width).min()?.min(MAX_VIDEO_WIDTH) & !1;
        let height = caps.iter().map(|c| c.max_height).min()?.min(MAX_VIDEO_HEIGHT) & !1;
        let fps = caps.iter().map(|c| c.max_fps).min()?.min(MAX_VIDEO_FPS);
        let bitrate = caps.iter().map(|c| c.max_bitrate_bps).min()?.min(TOTAL_MEDIA_BUDGET_BPS.saturating_sub(AUDIO_RESERVE_BPS));
        (width > 0 && height > 0 && fps > 0 && bitrate > 0).then_some(NegotiatedVideoV2 { codec, width, height, fps, bitrate_bps: bitrate, track_id })
    })
}

pub fn fallback_after_failure(failure: CodecFailure, h264_is_proven: bool, diagnostics: &mut FallbackDiagnostics) -> Option<VideoCodec> {
    diagnostics.record_failure(failure.reason);
    if h264_is_proven { diagnostics.record_h264_fallback(); Some(VideoCodec::H264) } else { None }
}

pub const fn advertised_layers() -> (u8, u8) { (ADVERTISED_SPATIAL_LAYERS, ADVERTISED_TEMPORAL_LAYERS) }

#[cfg(test)]
mod tests {
    use super::*;
    fn cap(codec: VideoCodec, w: u32, b: u32) -> VideoCodecCapability { VideoCodecCapability { codec, max_width: w, max_height: 720, max_fps: 30, max_bitrate_bps: b } }
    fn matrix(codecs: &[VideoCodec]) -> Vec<VideoCodecCapability> { codecs.iter().map(|c| cap(*c, 1280, 900_000)).collect() }
    #[test] fn prefers_av1_only_when_all_four_directions_support_it() { let av1 = matrix(&[VideoCodec::Av1, VideoCodec::H264]); let h264 = matrix(&[VideoCodec::H264]); assert_eq!(negotiate_video(&av1, &av1, &av1, &av1, 7).unwrap().codec, VideoCodec::Av1); assert_eq!(negotiate_video(&av1, &h264, &av1, &h264, 7).unwrap().codec, VideoCodec::H264); }
    #[test] fn rollout_modes_start_conservatively() { let caps = matrix(&[VideoCodec::Av1, VideoCodec::H264]); assert_eq!(negotiate_video_with_mode(&caps, &caps, &caps, &caps, 7, Av1RolloutMode::Auto).unwrap().codec, VideoCodec::H264); assert_eq!(negotiate_video_with_mode(&caps, &caps, &caps, &caps, 7, Av1RolloutMode::H264Only).unwrap().codec, VideoCodec::H264); }
    #[test] fn fallback_records_failure_and_selects_h264() { let mut d = FallbackDiagnostics::default(); assert_eq!(fallback_after_failure(CodecFailure { track_id: 8, reason: CodecFailureReason::DecodeFailure }, true, &mut d), Some(VideoCodec::H264)); assert_eq!(d.h264_fallbacks, 1); }
    #[test] fn clamps_rate_and_geometry_and_requires_new_track_for_fallback() { let caps = matrix(&[VideoCodec::H264]); let selected = negotiate_video(&caps, &caps, &caps, &caps, 9).unwrap(); assert_eq!((selected.width, selected.height, selected.bitrate_bps), (1280, 720, 800_000)); assert_eq!(TrackCodecState { track_id: 9, codec: VideoCodec::Av1 }.recover_from_init_failure(true), InitRecovery::RequiresNewTrack); }
    #[test] fn fallback_selects_h264_without_mutating_old_track() { let state = TrackCodecState { track_id: 4, codec: VideoCodec::Av1 }; assert_eq!(fallback_codec(state, true), Some(VideoCodec::H264)); assert_eq!(state.codec, VideoCodec::Av1); }
    #[test] fn advertises_l1t1_only() { assert_eq!(advertised_layers(), (1, 1)); }
}
