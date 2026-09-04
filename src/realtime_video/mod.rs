//! Codec-neutral primitives shared by realtime video producers and consumers.
//!
//! This module deliberately contains no transport, session policy, capture
//! backend, or codec-library dependency. Feature-specific adapters translate
//! these owned values to their local pipeline types.

use std::fmt;

/// Pixel layout accepted by the common pixel utilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Packed red, green, blue, alpha bytes.
    Rgba8,
    /// Packed blue, green, red, alpha bytes.
    Bgra8,
    /// Packed red, green, blue bytes.
    Rgb8,
}

impl PixelFormat {
    /// Number of bytes used by one pixel.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 | Self::Bgra8 => 4,
            Self::Rgb8 => 3,
        }
    }
}

/// Codec-independent stream configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecConfig {
    /// Encoded width in pixels.
    pub width: u32,
    /// Encoded height in pixels.
    pub height: u32,
    /// Target frame rate.
    pub fps: u32,
    /// Target bitrate in bits per second.
    pub bitrate_bps: u32,
    /// Maximum distance between keyframes.
    pub keyframe_interval: u64,
}

impl CodecConfig {
    /// Validate dimensions and rate knobs before a codec is constructed.
    pub fn validate(self) -> Result<Self, CodecError> {
        if self.width == 0 || self.height == 0 || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(CodecError::InvalidConfig(
                "dimensions must be non-zero even values",
            ));
        }
        if self.fps == 0 || self.bitrate_bps == 0 || self.keyframe_interval == 0 {
            return Err(CodecError::InvalidConfig(
                "rates and keyframe interval must be non-zero",
            ));
        }
        Ok(self)
    }
}

/// A captured frame owned by the realtime video boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// Capture timestamp in microseconds.
    pub timestamp_us: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Actual packed pixel layout.
    pub format: PixelFormat,
    /// Bytes per row.
    pub stride: usize,
    /// Pixel payload, including any row padding.
    pub pixels: Vec<u8>,
}

impl VideoFrame {
    /// Construct a tightly packed frame after checking its payload.
    pub fn packed(
        timestamp_us: u64,
        width: u32,
        height: u32,
        format: PixelFormat,
        pixels: Vec<u8>,
    ) -> Result<Self, CodecError> {
        let stride = checked_row_bytes(width, format)?;
        let expected = stride
            .checked_mul(height as usize)
            .ok_or(CodecError::SizeOverflow)?;
        if pixels.len() != expected {
            return Err(CodecError::InvalidFrame(
                "pixel payload does not match dimensions",
            ));
        }
        Ok(Self {
            timestamp_us,
            width,
            height,
            format,
            stride,
            pixels,
        })
    }
}

/// One encoded access unit independent of the implementation codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Codec identifier used for negotiation and diagnostics.
    pub codec: String,
    /// Encoded width.
    pub width: u32,
    /// Encoded height.
    pub height: u32,
    /// Source timestamp preserved through encoding.
    pub timestamp_us: u64,
    /// Whether this unit is independently decodable.
    pub keyframe: bool,
    /// Codec payload.
    pub bytes: Vec<u8>,
}

/// Stable metadata describing the active codec instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecMetadata {
    /// Codec identifier.
    pub codec: String,
    /// Active stream configuration.
    pub config: CodecConfig,
    /// Incremented when geometry/configuration is replaced.
    pub generation: u64,
}

/// Error classification shared by codec adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// Configuration values are unsupported.
    InvalidConfig(&'static str),
    /// A frame payload or dimensions are malformed.
    InvalidFrame(&'static str),
    /// Checked size arithmetic overflowed.
    SizeOverflow,
    /// The implementation could not initialize or process the frame.
    Backend,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid codec config: {message}"),
            Self::InvalidFrame(message) => write!(f, "invalid video frame: {message}"),
            Self::SizeOverflow => f.write_str("video frame size overflow"),
            Self::Backend => f.write_str("realtime video codec backend failure"),
        }
    }
}
impl std::error::Error for CodecError {}

/// Checked row-size calculation shared by all adapters.
pub fn checked_row_bytes(width: u32, format: PixelFormat) -> Result<usize, CodecError> {
    (width as usize)
        .checked_mul(format.bytes_per_pixel())
        .ok_or(CodecError::SizeOverflow)
}

/// Convert packed pixels to RGBA8, respecting row stride.
pub fn to_rgba8(frame: &VideoFrame) -> Result<Vec<u8>, CodecError> {
    let row_bytes = checked_row_bytes(frame.width, frame.format)?;
    let required = frame
        .stride
        .checked_mul(frame.height as usize)
        .ok_or(CodecError::SizeOverflow)?;
    if frame.stride < row_bytes || frame.pixels.len() != required {
        return Err(CodecError::InvalidFrame("stride or payload is invalid"));
    }
    let mut out = Vec::with_capacity(
        (frame.width as usize)
            .checked_mul(frame.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(CodecError::SizeOverflow)?,
    );
    for row in frame.pixels.chunks_exact(frame.stride) {
        for pixel in row[..row_bytes].chunks_exact(frame.format.bytes_per_pixel()) {
            match frame.format {
                PixelFormat::Rgba8 => out.extend_from_slice(pixel),
                PixelFormat::Bgra8 => {
                    out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]])
                }
                PixelFormat::Rgb8 => out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
            }
        }
    }
    Ok(out)
}

/// Nearest-neighbour scale for packed RGBA8 pixels.
pub fn scale_rgba8(
    pixels: &[u8],
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CodecError> {
    let source_len = (source_width as usize)
        .checked_mul(source_height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(CodecError::SizeOverflow)?;
    let output_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(CodecError::SizeOverflow)?;
    if source_width == 0
        || source_height == 0
        || width == 0
        || height == 0
        || pixels.len() != source_len
    {
        return Err(CodecError::InvalidFrame(
            "RGBA dimensions or payload are invalid",
        ));
    }
    let mut out = vec![0; output_len];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let sx = x * source_width as usize / width as usize;
            let sy = y * source_height as usize / height as usize;
            let from = (sy * source_width as usize + sx) * 4;
            let to = (y * width as usize + x) * 4;
            out[to..to + 4].copy_from_slice(&pixels[from..from + 4]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel_conversion_handles_bgra_and_stride() {
        let frame = VideoFrame {
            timestamp_us: 7,
            width: 1,
            height: 1,
            format: PixelFormat::Bgra8,
            stride: 8,
            pixels: vec![3, 2, 1, 255, 9, 9, 9, 9],
        };
        assert_eq!(to_rgba8(&frame).unwrap(), vec![1, 2, 3, 255]);
    }
    #[test]
    fn scaler_preserves_corners() {
        let input = vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255];
        let output = scale_rgba8(&input, 2, 2, 4, 4).unwrap();
        assert_eq!(&output[0..4], &[1, 0, 0, 255]);
        assert_eq!(&output[12..16], &[2, 0, 0, 255]);
        assert_eq!(&output[48..52], &[3, 0, 0, 255]);
        assert_eq!(&output[60..64], &[4, 0, 0, 255]);
    }
}
