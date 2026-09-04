//! Checked conversion from camera RGB frames to negotiated video geometry.

use anyhow::{anyhow, Result};

use super::{codec::RawVideoFrame, RawCaptureFrame};

/// Policy used when source and target aspect ratios differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectPolicy {
    /// Preserve every source pixel and fill unused target pixels with black.
    Letterbox,
    /// Preserve aspect ratio while cropping the excess source area.
    Crop,
}

/// Convert a strided RGB capture frame to packed RGB at the target geometry.
pub fn convert_frame(
    source: &RawCaptureFrame,
    target_width: u32,
    target_height: u32,
    policy: AspectPolicy,
) -> Result<RawVideoFrame> {
    if target_width == 0 || target_height == 0 {
        return Err(anyhow!("target dimensions must be non-zero"));
    }
    if source.width == 0 || source.height == 0 {
        return Err(anyhow!("source dimensions must be non-zero"));
    }
    let source_row = (source.width as usize)
        .checked_mul(3)
        .ok_or_else(|| anyhow!("source row size overflow"))?;
    if source.stride < source_row {
        return Err(anyhow!("source stride is smaller than packed RGB row"));
    }
    let source_len = source
        .stride
        .checked_mul(source.height as usize)
        .ok_or_else(|| anyhow!("source frame size overflow"))?;
    if source.data.len() < source_len {
        return Err(anyhow!("source frame is shorter than stride × height"));
    }
    let output_len = (target_width as usize)
        .checked_mul(target_height as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| anyhow!("target frame size overflow"))?;
    let (scaled_w, scaled_h) = match policy {
        AspectPolicy::Letterbox => fit(source.width, source.height, target_width, target_height),
        AspectPolicy::Crop => cover(source.width, source.height, target_width, target_height),
    };
    let mut output = vec![0; output_len];
    let offset_x = (target_width - scaled_w) / 2;
    let offset_y = (target_height - scaled_h) / 2;
    let crop_x = if policy == AspectPolicy::Crop { (scaled_w - target_width) / 2 } else { 0 };
    let crop_y = if policy == AspectPolicy::Crop { (scaled_h - target_height) / 2 } else { 0 };
    for y in 0..target_height {
        for x in 0..target_width {
            let (dx, dy) = if policy == AspectPolicy::Letterbox {
                (x.saturating_sub(offset_x), y.saturating_sub(offset_y))
            } else {
                (x + crop_x, y + crop_y)
            };
            if policy == AspectPolicy::Letterbox && (x < offset_x || y < offset_y || x >= offset_x + scaled_w || y >= offset_y + scaled_h) { continue; }
            let sx = (dx as u64 * source.width as u64 / scaled_w as u64) as usize;
            let sy = (dy as u64 * source.height as u64 / scaled_h as u64) as usize;
            let src = sy * source.stride + sx * 3;
            let dst = (y as usize * target_width as usize + x as usize) * 3;
            output[dst..dst + 3].copy_from_slice(&source.data[src..src + 3]);
        }
    }
    Ok(RawVideoFrame { width: target_width, height: target_height, timestamp_us: source.timestamp_us, rgb: output })
}

fn fit(sw: u32, sh: u32, tw: u32, th: u32) -> (u32, u32) {
    if sw as u64 * th as u64 >= sh as u64 * tw as u64 {
        (tw, (sh as u64 * tw as u64 / sw as u64) as u32)
    } else {
        ((sw as u64 * th as u64 / sh as u64) as u32, th)
    }
}

fn cover(sw: u32, sh: u32, tw: u32, th: u32) -> (u32, u32) {
    if sw as u64 * th as u64 <= sh as u64 * tw as u64 {
        (tw, (sh as u64 * tw as u64 / sw as u64) as u32)
    } else {
        ((sw as u64 * th as u64 / sh as u64) as u32, th)
    }
}
