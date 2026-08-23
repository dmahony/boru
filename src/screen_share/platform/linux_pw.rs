//! Pure PipeWire format negotiation and CPU frame normalization (BORU-SS-14).
//!
//! This module holds the parts of the Linux ScreenCast ingestion that do not
//! need a live PipeWire session, so they are unit-testable headless:
//!
//! - The SPA pod constants, the format advertisement pod builder
//!   ([`build_format_pod`]), and the negotiated-format pod parser
//!   ([`parse_format_pod`]).
//! - The mapping from negotiated SPA video format ids to the CPU byte layout
//!   ([`PwPixelLayout`]) and Boru's normalized [`PixelFormat`].
//! - The CPU-mapped buffer normalization ([`normalize_buffer`]) that copies a
//!   PipeWire `spa_data` payload row-by-row into a tightly packed BGRA8/RGBA8
//!   frame, honouring the chunk stride and expanding 24-bit RGB/BGR to 4 bytes
//!   per pixel.
//!
//! Constants are taken from PipeWire's own headers (MIT-licensed):
//! `spa/include/spa/utils/type.h`, `spa/include/spa/pod/pod.h`,
//! `spa/include/spa/param/format.h`, `spa/include/spa/param/param-types.h`,
//! `spa/include/spa/param/video/raw.h`, and
//! `spa/include/spa/buffer/buffer.h`. No RustDesk code was consulted.
//!
//! ## DMA-BUF readiness
//!
//! The current path is CPU-mapped only: `pw_stream_connect` is called with
//! `PW_STREAM_FLAG_MAP_BUFFERS`, so every `spa_data.data` pointer is a valid
//! CPU address and [`normalize_buffer`] copies it. A future DMA-BUF path keeps
//! the same negotiation and [`NegotiatedFormat`] contract but drops
//! `MAP_BUFFERS`, reads the `SPA_DATA_DmaBuf` data type (3) and the `fd`
//! field, and delivers a [`CapturedFrame`](crate::screen_share::CapturedFrame)
//! with `gpu_handle` set instead of CPU pixels — the pod constants, format
//! mapping, and renegotiation logic in this module are shared by both paths.
//!
//! ## Damage metadata (`spa_meta_region`) — intentionally not consumed
//!
//! PipeWire buffers can carry a `SPA_META_Region` metadata chunk describing
//! which region of the frame changed since the previous buffer, which would
//! let the portal path attach a [`DirtyRegion`](crate::screen_share::DirtyRegion)
//! like the X11 damage path does. In practice the xdg-desktop-portal
//! ScreenCast stream (which is what this backend negotiates) does not emit
//! per-buffer damage: the portal composites and delivers complete frames, and
//! the `spa_meta_region` metadata is only produced by compositors that opt
//! into explicit damage signalling for their own streams. Consuming it would
//! therefore add dlopen plumbing for a metadata type the portal almost never
//! provides, and the frames would still have to be treated as full-frame in
//! the common case. Decision (BORU-SS-32): the portal/PipeWire path stays
//! full-frame (`dirty_region: None`); damage-awareness is delivered by the
//! direct X11 backend, which owns the pixel pipeline and the damage
//! extension.

// SPA constants intentionally mirror PipeWire's C enum names (RGBx, BGRx,
// SPA_TYPE_OBJECT_Format, ...) so they can be cross-checked against the
// upstream headers; the `non_upper_case_globals` lint is suppressed for them.
#![allow(non_upper_case_globals)]

use crate::screen_share::coords::CursorSprite;
use crate::screen_share::{PixelFormat, ScreenShareError};

// ── SPA constants (values from PipeWire's headers, see module docs) ────────

/// `SPA_TYPE_Id` — an enum/id value pod (`spa/utils/type.h`).
pub(crate) const SPA_TYPE_Id: u32 = 2;
/// `SPA_TYPE_Rectangle` — a `{width, height}` pod.
pub(crate) const SPA_TYPE_Rectangle: u32 = 9;
/// `SPA_TYPE_Object` — an object pod (format objects are objects).
pub(crate) const SPA_TYPE_Object: u32 = 14;
/// `SPA_TYPE_Choice` — a choice pod (enum of alternatives).
pub(crate) const SPA_TYPE_Choice: u32 = 18;

/// `SPA_TYPE_OBJECT_Format` — the object type discriminator written in the
/// body of a format object (`spa/utils/type.h`, `SPA_TYPE_OBJECT_START + 3`).
pub(crate) const SPA_TYPE_OBJECT_Format: u32 = 0x40003;

/// `SPA_FORMAT_mediaType` — media type property key (`spa/param/format.h`).
pub(crate) const SPA_FORMAT_mediaType: u32 = 1;
/// `SPA_FORMAT_mediaSubtype` — media subtype property key.
pub(crate) const SPA_FORMAT_mediaSubtype: u32 = 2;
/// `SPA_FORMAT_VIDEO_format` — video format property key.
pub(crate) const SPA_FORMAT_VIDEO_format: u32 = 0x20001;
/// `SPA_FORMAT_VIDEO_size` — video size property key.
pub(crate) const SPA_FORMAT_VIDEO_size: u32 = 0x20003;

/// `SPA_MEDIA_TYPE_VIDEO` — video media type id (`spa/param/format.h`).
pub(crate) const SPA_MEDIA_TYPE_VIDEO: u32 = 2;
/// `SPA_MEDIA_SUBTYPE_RAW` — raw (uncompressed) media subtype id.
pub(crate) const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;

/// `SPA_VIDEO_FORMAT_RGBx` — 32-bit packed RGB with unused last byte
/// (`spa/param/video/raw.h`).
pub(crate) const SPA_VIDEO_FORMAT_RGBx: u32 = 7;
/// `SPA_VIDEO_FORMAT_BGRx` — 32-bit packed BGR with unused last byte.
pub(crate) const SPA_VIDEO_FORMAT_BGRx: u32 = 8;
/// `SPA_VIDEO_FORMAT_RGBA` — 32-bit RGBA.
pub(crate) const SPA_VIDEO_FORMAT_RGBA: u32 = 11;
/// `SPA_VIDEO_FORMAT_BGRA` — 32-bit BGRA.
pub(crate) const SPA_VIDEO_FORMAT_BGRA: u32 = 12;
/// `SPA_VIDEO_FORMAT_RGB` — 24-bit packed RGB (3 bytes per pixel).
pub(crate) const SPA_VIDEO_FORMAT_RGB: u32 = 15;
/// `SPA_VIDEO_FORMAT_BGR` — 24-bit packed BGR (3 bytes per pixel).
pub(crate) const SPA_VIDEO_FORMAT_BGR: u32 = 16;

/// `SPA_PARAM_Format` — the format parameter id (`spa/param/param-types.h`).
pub(crate) const SPA_PARAM_Format: u32 = 4;
/// `SPA_PARAM_Buffers` — the buffers parameter id.
pub(crate) const SPA_PARAM_Buffers: u32 = 5;

/// `SPA_CHOICE_Enum` — choice kind for a list of alternatives
/// (`spa/pod/pod.h`; the first value is the default).
pub(crate) const SPA_CHOICE_Enum: u32 = 3;

// `spa/buffer/buffer.h` also defines `SPA_DATA_MemPtr` (1) and
// `SPA_DATA_DmaBuf` (3). The CPU path here always maps buffers
// (PW_STREAM_FLAG_MAP_BUFFERS), so those data types are not referenced; a
// future DMA-BUF path will read `spa_data.type == SPA_DATA_DmaBuf (3)` and
// the `fd` field.

/// How a negotiated SPA video format is laid out in memory. Four bytes per
/// pixel for the 32-bit formats, three for the 24-bit packed formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PwPixelLayout {
    /// 8-bit BGRA, four bytes per pixel.
    Bgra8,
    /// 8-bit RGBA, four bytes per pixel.
    Rgba8,
    /// 24-bit packed BGR, three bytes per pixel (BGR24).
    Bgr24,
    /// 24-bit packed RGB, three bytes per pixel (RGB24).
    Rgb24,
}

impl PwPixelLayout {
    /// Bytes per pixel on the wire.
    pub(crate) fn bytes_per_pixel(self) -> u32 {
        match self {
            PwPixelLayout::Bgra8 | PwPixelLayout::Rgba8 => 4,
            PwPixelLayout::Bgr24 | PwPixelLayout::Rgb24 => 3,
        }
    }

    /// The normalized [`PixelFormat`] this layout is converted into for the
    /// encoder boundary. Channel order is preserved; the alpha byte added
    /// during 24-bit expansion is 255 (opaque).
    pub(crate) fn to_pixel_format(self) -> PixelFormat {
        match self {
            PwPixelLayout::Bgra8 | PwPixelLayout::Bgr24 => PixelFormat::Bgra8,
            PwPixelLayout::Rgba8 | PwPixelLayout::Rgb24 => PixelFormat::Rgba8,
        }
    }
}

/// Map a negotiated SPA video format id to its CPU byte layout. `None` for
/// formats the CPU path does not consume (YUV, 10-bit, planar, etc.).
pub(crate) fn layout_from_spa_format_id(format_id: u32) -> Option<PwPixelLayout> {
    match format_id {
        SPA_VIDEO_FORMAT_BGRx | SPA_VIDEO_FORMAT_BGRA => Some(PwPixelLayout::Bgra8),
        SPA_VIDEO_FORMAT_RGBx | SPA_VIDEO_FORMAT_RGBA => Some(PwPixelLayout::Rgba8),
        SPA_VIDEO_FORMAT_BGR => Some(PwPixelLayout::Bgr24),
        SPA_VIDEO_FORMAT_RGB => Some(PwPixelLayout::Rgb24),
        _ => None,
    }
}

/// The formats the capture stream advertises, in preference order. The first
/// entry is the default the portal should pick when it supports it. BGRx is
/// first because xdg-desktop-portal backends commonly stream BGRx; the 24-bit
/// packed RGB/BGR formats are advertised as the lowest preference.
pub(crate) fn advertised_format_ids() -> &'static [u32] {
    &[
        SPA_VIDEO_FORMAT_BGRx,
        SPA_VIDEO_FORMAT_RGBx,
        SPA_VIDEO_FORMAT_BGRA,
        SPA_VIDEO_FORMAT_RGBA,
        SPA_VIDEO_FORMAT_BGR,
        SPA_VIDEO_FORMAT_RGB,
    ]
}

/// The negotiated stream geometry and pixel layout, shared between the
/// PipeWire capture thread and the capture consumer.
///
/// `generation` is bumped by the stream callback on every real renegotiation
/// (a different geometry/format), so consumers can distinguish "repeated
/// callback for the same format" from "the display resolution changed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NegotiatedFormat {
    pub width: u32,
    pub height: u32,
    /// The wire byte layout of the negotiated SPA format. Determines the
    /// bytes-per-pixel used to copy buffers and the normalized
    /// [`PixelFormat`] delivered to the encoder boundary.
    pub layout: PwPixelLayout,
    pub generation: u64,
}

impl Default for NegotiatedFormat {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            layout: PwPixelLayout::Bgra8,
            generation: 0,
        }
    }
}

/// Build the SPA format object pod advertising the supported RGB/BGR formats.
///
/// Layout (all little-endian, 8-byte aligned; see `spa/pod/builder.h`):
///
/// ```text
/// pod header     { u32 body_size, u32 type = SPA_TYPE_Object }
/// object body    { u32 object_type = SPA_TYPE_OBJECT_Format,
///                  u32 id = SPA_PARAM_Format }
/// prop           { u32 key, u32 flags, pod value }
/// ```
///
/// The format property is a [`SPA_TYPE_Choice`] of `SPA_CHOICE_Enum` with the
/// advertised format ids; the size property is a plain [`SPA_TYPE_Rectangle`]
/// hint that the portal overrides with the real monitor size.
pub(crate) fn build_format_pod() -> Vec<u8> {
    let mut pod: Vec<u8> = Vec::new();
    // Placeholder header: size patched once the body is known.
    pod.extend_from_slice(&[0, 0, 0, 0]);
    pod.extend_from_slice(&SPA_TYPE_Object.to_le_bytes());
    pod.extend_from_slice(&SPA_TYPE_OBJECT_Format.to_le_bytes());
    pod.extend_from_slice(&SPA_PARAM_Format.to_le_bytes());
    push_prop_id(&mut pod, SPA_FORMAT_mediaType, SPA_MEDIA_TYPE_VIDEO);
    push_prop_id(&mut pod, SPA_FORMAT_mediaSubtype, SPA_MEDIA_SUBTYPE_RAW);
    push_prop_choice_id(&mut pod, SPA_FORMAT_VIDEO_format, advertised_format_ids());
    push_prop_rectangle(&mut pod, SPA_FORMAT_VIDEO_size, 640, 360);
    let body_size = pod.len() as u32 - 8;
    pod[0..4].copy_from_slice(&body_size.to_le_bytes());
    pod
}

fn push_prop_id(pod: &mut Vec<u8>, key: u32, value: u32) {
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&4u32.to_le_bytes()); // value pod body size
    pod.extend_from_slice(&SPA_TYPE_Id.to_le_bytes());
    pod.extend_from_slice(&value.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // value padding (8-byte align)
}

fn push_prop_rectangle(pod: &mut Vec<u8>, key: u32, width: u32, height: u32) {
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&8u32.to_le_bytes()); // value pod body size
    pod.extend_from_slice(&SPA_TYPE_Rectangle.to_le_bytes());
    pod.extend_from_slice(&width.to_le_bytes());
    pod.extend_from_slice(&height.to_le_bytes());
}

fn push_prop_choice_id(pod: &mut Vec<u8>, key: u32, values: &[u32]) {
    let n = values.len();
    // Choice body: choice type (4) + flags (4) + child pod header (8) +
    // n Id values (4 each). The child is an Id pod whose "size" is 4 (the
    // value body); the alternatives follow the child value.
    let value_body = 16 + 4 * n;
    pod.extend_from_slice(&key.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // flags
    pod.extend_from_slice(&(value_body as u32).to_le_bytes());
    pod.extend_from_slice(&SPA_TYPE_Choice.to_le_bytes());
    pod.extend_from_slice(&SPA_CHOICE_Enum.to_le_bytes());
    pod.extend_from_slice(&0u32.to_le_bytes()); // choice flags
    pod.extend_from_slice(&4u32.to_le_bytes()); // child pod size
    pod.extend_from_slice(&SPA_TYPE_Id.to_le_bytes());
    pod.extend_from_slice(&values[0].to_le_bytes()); // default = first format
    for v in &values[1..] {
        pod.extend_from_slice(&v.to_le_bytes());
    }
    // The value pod is 8-byte aligned before the next property.
    while !pod.len().is_multiple_of(8) {
        pod.push(0);
    }
}

/// Parse a SPA format object pod into `(width, height, pixel_layout)`.
///
/// Accepts both the pods built by [`build_format_pod`] and the negotiated
/// format pods a real PipeWire stream delivers (the property values may be a
/// plain `Id`/`Rectangle` or a `Choice` wrapping them — the first/default
/// value is used). Returns `None` for malformed pods and for formats the CPU
/// path does not consume.
pub(crate) fn parse_format_pod(bytes: &[u8]) -> Option<(u32, u32, PwPixelLayout)> {
    if bytes.len() < 8 {
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != SPA_TYPE_Object {
        return None;
    }
    let total = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    // Clamp reads to the declared pod body so a short pod cannot overrun.
    let body = bytes.get(8..8 + total)?;
    if body.len() < 8 {
        return None;
    }
    // body[0..4] = object type (SPA_TYPE_OBJECT_Format), body[4..8] = id;
    // properties follow.
    let mut offset = 8usize;
    let mut format_id: Option<u32> = None;
    let mut size: Option<(u32, u32)> = None;
    while offset + 16 <= body.len() {
        let key = u32::from_le_bytes(body[offset..offset + 4].try_into().ok()?);
        let value_body_size =
            u32::from_le_bytes(body[offset + 8..offset + 12].try_into().ok()?) as usize;
        let value_type = u32::from_le_bytes(body[offset + 12..offset + 16].try_into().ok()?);
        // Value data starts after the key+flags (8) and the value pod header
        // (size+type, 8): a Choice's body starts with its own type+flags
        // before the child pod, so the child type/value sit at fixed offsets.
        let value_data = &body[offset + 16..];
        match (key, value_type) {
            (SPA_FORMAT_VIDEO_format, SPA_TYPE_Choice) => {
                // choice body: type(4) flags(4) child pod(size, type, value)
                if value_body_size >= 20 && value_data.len() >= 20 {
                    let child_type = u32::from_le_bytes(value_data[12..16].try_into().ok()?);
                    if child_type == SPA_TYPE_Id {
                        format_id = Some(u32::from_le_bytes(value_data[16..20].try_into().ok()?));
                    }
                }
            }
            (SPA_FORMAT_VIDEO_format, SPA_TYPE_Id) => {
                if value_data.len() >= 4 {
                    format_id = Some(u32::from_le_bytes(value_data[0..4].try_into().ok()?));
                }
            }
            (SPA_FORMAT_VIDEO_size, SPA_TYPE_Rectangle) if value_data.len() >= 8 => {
                let w = u32::from_le_bytes(value_data[0..4].try_into().ok()?);
                let h = u32::from_le_bytes(value_data[4..8].try_into().ok()?);
                size = Some((w, h));
            }
            _ => {}
        }
        let value_pod_size = (8 + value_body_size + 7) & !7;
        offset += 8 + value_pod_size;
    }
    let format_id = format_id?;
    let (width, height) = size?;
    let layout = layout_from_spa_format_id(format_id)?;
    Some((width, height, layout))
}

/// Copy a CPU-mapped PipeWire buffer into a tightly packed 4-byte-per-pixel
/// frame, honouring the chunk row stride and expanding 24-bit layouts.
///
/// `src` is the mapped `spa_data.data` slice, `offset` the chunk offset,
/// `src_stride` the chunk row stride in bytes (`<= 0` means tightly packed).
/// Row padding is dropped so the returned buffer matches
/// `CapturedFrame::cpu`'s tight layout (the encoder requires
/// `pixels.len() == width * height * 4`).
pub(crate) fn normalize_buffer(
    src: &[u8],
    offset: usize,
    width: u32,
    height: u32,
    layout: PwPixelLayout,
    src_stride: i32,
) -> Result<Vec<u8>, ScreenShareError> {
    let bpp = layout.bytes_per_pixel() as usize;
    let tight = (width as usize)
        .checked_mul(bpp)
        .ok_or_else(|| ScreenShareError::stream("frame dimensions overflow"))?;
    let stride = if src_stride > 0 {
        src_stride as usize
    } else {
        tight
    };
    if stride < tight {
        return Err(ScreenShareError::stream(format!(
            "pipewire row stride {stride} is smaller than width * bytes-per-pixel ({tight})"
        )));
    }
    let needed = stride
        .checked_mul(height as usize)
        .ok_or_else(|| ScreenShareError::stream("frame dimensions overflow"))?;
    let end = offset
        .checked_add(needed)
        .ok_or_else(|| ScreenShareError::stream("pipewire buffer offset overflow"))?;
    if end > src.len() {
        return Err(ScreenShareError::stream(format!(
            "pipewire buffer too small: need {needed} bytes at offset {offset}, have {}",
            src.len()
        )));
    }
    let out_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ScreenShareError::stream("frame dimensions overflow"))?;

    let mut out = Vec::with_capacity(out_len);
    let mut row = offset;
    match layout {
        PwPixelLayout::Bgra8 | PwPixelLayout::Rgba8 => {
            for _ in 0..height {
                out.extend_from_slice(&src[row..row + tight]);
                row += stride;
            }
        }
        PwPixelLayout::Bgr24 | PwPixelLayout::Rgb24 => {
            let row_bytes = width as usize * 3;
            for _ in 0..height {
                let pixels = &src[row..row + row_bytes];
                for pixel in pixels.chunks_exact(3) {
                    out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
                row += stride;
            }
        }
    }
    Ok(out)
}

// ── spa_meta_cursor parsing (BORU-SS-33 / PDF Task 5.3) ───────────────────
//
// When the portal runs in `Metadata` cursor mode (4), the compositor does
// NOT bake the cursor into the PipeWire buffers. Instead each buffer carries
// a `spa_meta_cursor` meta (type `SPA_META_Cursor` = 5) whose data is laid
// out exactly as the upstream C structs (spa/include/spa/buffer/meta.h):
//
//   struct spa_meta_cursor {
//       uint32_t id;             // 0 = no new cursor data
//       uint32_t flags;          // SPA_META_CURSOR_FLAG_HIDE = 1
//       struct spa_point position; // { int32 x, y } in stream coordinates
//       struct spa_point hotspot;  // { int32 x, y }
//       uint32_t bitmap_offset;    // offset of spa_meta_bitmap, 0 = none
//   };
//   struct spa_meta_bitmap {
//       uint32_t format;         // spa_video_format (ARGB = 13)
//       struct spa_rectangle size; // { uint32 width, height }
//       int32_t stride;
//       uint32_t offset;         // offset of bitmap data in this struct
//   };
//
// The bitmap is ARGB8888 (the portal cursor format); Boru's shared
// [`CursorSprite`](crate::screen_share::CursorSprite) is BGRA8, so the
// parser swaps R/B while copying.

/// `SPA_META_Cursor` buffer meta type id (`spa/include/spa/buffer/meta.h`).
pub(crate) const SPA_META_Cursor: u32 = 5;
/// `SPA_META_CURSOR_FLAG_HIDE` — cursor hidden flag value.
pub(crate) const SPA_META_CURSOR_FLAG_HIDE: u32 = 1;
/// `SPA_VIDEO_FORMAT_ARGB` — cursor bitmaps are ARGB8888.
pub(crate) const SPA_VIDEO_FORMAT_ARGB: u32 = 13;

/// Parsed `spa_meta_cursor` from one PipeWire buffer (BORU-SS-33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSpaCursor {
    /// Cursor position in stream (source) coordinates, physical pixels.
    pub x: i32,
    /// Cursor position in stream (source) coordinates, physical pixels.
    pub y: i32,
    /// Whether the cursor is visible (`SPA_META_CURSOR_FLAG_HIDE` unset).
    pub visible: bool,
    /// Sprite pixels as BGRA8 (`width * height * 4`) when a bitmap was
    /// attached in this buffer; `None` for position-only updates.
    pub sprite: Option<CursorSprite>,
}

/// Parse a `spa_meta_cursor` data blob (the bytes pointed to by a
/// `SPA_META_Cursor` meta). Returns `None` when the blob is too short or
/// reports no new cursor data (`id == 0`). The bitmap, when present, is
/// converted from ARGB8888 to BGRA8 so it can ride the shared
/// [`CursorSprite`](crate::screen_share::CursorSprite) type directly.
pub(crate) fn parse_spa_cursor_meta(bytes: &[u8]) -> Option<ParsedSpaCursor> {
    if bytes.len() < 28 {
        return None;
    }
    let id = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    if id == 0 {
        return None;
    }
    let flags = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let x = i32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let y = i32::from_le_bytes(bytes[12..16].try_into().ok()?);
    let bitmap_offset = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    let visible = flags & SPA_META_CURSOR_FLAG_HIDE == 0;

    let sprite = parse_spa_cursor_bitmap(bytes, bitmap_offset)?;
    Some(ParsedSpaCursor {
        x,
        y,
        visible,
        sprite,
    })
}

/// Parse the `spa_meta_bitmap` pointed to by `bitmap_offset` inside the
/// cursor meta blob. Returns `None` when there is no bitmap (position-only
/// update) or the bitmap is malformed.
fn parse_spa_cursor_bitmap(bytes: &[u8], bitmap_offset: u32) -> Option<Option<CursorSprite>> {
    if bitmap_offset == 0 {
        return Some(None);
    }
    let start = bitmap_offset as usize;
    // spa_meta_bitmap = format(4) + size(8) + stride(4) + offset(4) = 20.
    if start + 20 > bytes.len() {
        return None;
    }
    let format = u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
    let width = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[start + 8..start + 12].try_into().ok()?);
    let stride = i32::from_le_bytes(bytes[start + 12..start + 16].try_into().ok()?);
    let data_offset = u32::from_le_bytes(bytes[start + 16..start + 20].try_into().ok()?);
    if width == 0 || height == 0 || width > 128 || height > 128 {
        return None;
    }
    if data_offset == 0 || data_offset < 20 {
        return Some(None);
    }
    let data_start = start.checked_add(data_offset as usize)?;
    let row_stride = if stride > 0 {
        stride as usize
    } else {
        width as usize * 4
    };
    let needed = row_stride.checked_mul(height as usize)?;
    if data_start.checked_add(needed)? > bytes.len() {
        return None;
    }
    // Cursor bitmaps are ARGB8888 (portal contract). Convert ARGB → BGRA:
    // source bytes [A,R,G,B] → BGRA [B,G,R,A]. BGRA (12) passes through.
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for row in 0..height as usize {
        let row_start = data_start + row * row_stride;
        for col in 0..width as usize {
            let i = row_start + col * 4;
            let (a, r, g, b) = match format {
                SPA_VIDEO_FORMAT_ARGB => (bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]),
                SPA_VIDEO_FORMAT_BGRA => (bytes[i + 3], bytes[i + 2], bytes[i + 1], bytes[i]),
                _ => return None,
            };
            pixels.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(Some(CursorSprite {
        width,
        height,
        hotspot_x: 0,
        hotspot_y: 0,
        pixels,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32le(v: u32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    /// Build a pod the way a real PipeWire stream would send a negotiated
    /// format: mediaType/mediaSubtype as plain Ids, the video format as a
    /// plain Id (no choice wrapper), and the size as a Rectangle.
    fn real_negotiated_pod(format_id: u32, width: u32, height: u32) -> Vec<u8> {
        let mut pod: Vec<u8> = Vec::new();
        pod.extend_from_slice(&[0, 0, 0, 0]);
        pod.extend_from_slice(&SPA_TYPE_Object.to_le_bytes());
        pod.extend_from_slice(&SPA_TYPE_OBJECT_Format.to_le_bytes());
        pod.extend_from_slice(&SPA_PARAM_Format.to_le_bytes());
        push_prop_id(&mut pod, SPA_FORMAT_mediaType, SPA_MEDIA_TYPE_VIDEO);
        push_prop_id(&mut pod, SPA_FORMAT_mediaSubtype, SPA_MEDIA_SUBTYPE_RAW);
        push_prop_id(&mut pod, SPA_FORMAT_VIDEO_format, format_id);
        push_prop_rectangle(&mut pod, SPA_FORMAT_VIDEO_size, width, height);
        let body_size = pod.len() as u32 - 8;
        pod[0..4].copy_from_slice(&body_size.to_le_bytes());
        pod
    }

    #[test]
    fn constants_match_pipewire_headers() {
        // Guard the ABI-critical values against future edits (they come from
        // PipeWire's MIT headers; a wrong constant silently breaks the whole
        // stream).
        assert_eq!(SPA_TYPE_Id, 2);
        assert_eq!(SPA_TYPE_Rectangle, 9);
        assert_eq!(SPA_TYPE_Object, 14);
        assert_eq!(SPA_TYPE_Choice, 18);
        assert_eq!(SPA_TYPE_OBJECT_Format, 0x40003);
        assert_eq!(SPA_MEDIA_TYPE_VIDEO, 2);
        assert_eq!(SPA_MEDIA_SUBTYPE_RAW, 1);
        assert_eq!(SPA_VIDEO_FORMAT_RGBx, 7);
        assert_eq!(SPA_VIDEO_FORMAT_BGRx, 8);
        assert_eq!(SPA_VIDEO_FORMAT_RGBA, 11);
        assert_eq!(SPA_VIDEO_FORMAT_BGRA, 12);
        assert_eq!(SPA_VIDEO_FORMAT_RGB, 15);
        assert_eq!(SPA_VIDEO_FORMAT_BGR, 16);
        assert_eq!(SPA_PARAM_Format, 4);
        assert_eq!(SPA_PARAM_Buffers, 5);
        assert_eq!(SPA_CHOICE_Enum, 3);
        // SPA_DATA_* values (MemPtr=1, DmaBuf=3) are intentionally not
        // constants here — see the comment above the SPA_CHOICE_Enum block.
    }

    #[test]
    fn advertised_formats_cover_common_rgb_bgr() {
        assert_eq!(
            advertised_format_ids(),
            &[
                SPA_VIDEO_FORMAT_BGRx,
                SPA_VIDEO_FORMAT_RGBx,
                SPA_VIDEO_FORMAT_BGRA,
                SPA_VIDEO_FORMAT_RGBA,
                SPA_VIDEO_FORMAT_BGR,
                SPA_VIDEO_FORMAT_RGB,
            ]
        );
    }

    #[test]
    fn layout_mapping_covers_advertised_formats() {
        for id in advertised_format_ids() {
            assert!(
                layout_from_spa_format_id(*id).is_some(),
                "format {id} not mapped"
            );
        }
        assert_eq!(
            layout_from_spa_format_id(SPA_VIDEO_FORMAT_BGRx),
            Some(PwPixelLayout::Bgra8)
        );
        assert_eq!(
            layout_from_spa_format_id(SPA_VIDEO_FORMAT_BGRA),
            Some(PwPixelLayout::Bgra8)
        );
        assert_eq!(
            layout_from_spa_format_id(SPA_VIDEO_FORMAT_RGBx),
            Some(PwPixelLayout::Rgba8)
        );
        assert_eq!(
            layout_from_spa_format_id(SPA_VIDEO_FORMAT_RGBA),
            Some(PwPixelLayout::Rgba8)
        );
        assert_eq!(
            layout_from_spa_format_id(SPA_VIDEO_FORMAT_BGR),
            Some(PwPixelLayout::Bgr24)
        );
        assert_eq!(
            layout_from_spa_format_id(SPA_VIDEO_FORMAT_RGB),
            Some(PwPixelLayout::Rgb24)
        );
        // YUV / unknown formats are not consumable by the CPU path.
        assert_eq!(layout_from_spa_format_id(2), None); // I420
        assert_eq!(layout_from_spa_format_id(0), None);
        assert_eq!(layout_from_spa_format_id(1), None);
        assert_eq!(layout_from_spa_format_id(u32::MAX), None);
        // 24-bit layouts map to the matching 4-byte normalized format.
        assert_eq!(PwPixelLayout::Bgr24.to_pixel_format(), PixelFormat::Bgra8);
        assert_eq!(PwPixelLayout::Rgb24.to_pixel_format(), PixelFormat::Rgba8);
        assert_eq!(PwPixelLayout::Bgra8.to_pixel_format(), PixelFormat::Bgra8);
        assert_eq!(PwPixelLayout::Rgba8.to_pixel_format(), PixelFormat::Rgba8);
        assert_eq!(PwPixelLayout::Bgr24.bytes_per_pixel(), 3);
        assert_eq!(PwPixelLayout::Rgb24.bytes_per_pixel(), 3);
        assert_eq!(PwPixelLayout::Bgra8.bytes_per_pixel(), 4);
    }

    #[test]
    fn build_format_pod_is_valid_object_and_parses_to_default() {
        let pod = build_format_pod();
        assert!(pod.len() > 32);
        // Header: body size + Object type.
        assert_eq!(
            u32::from_le_bytes(pod[4..8].try_into().unwrap()),
            SPA_TYPE_Object
        );
        // Object body discriminator + param id.
        assert_eq!(
            u32::from_le_bytes(pod[8..12].try_into().unwrap()),
            SPA_TYPE_OBJECT_Format
        );
        assert_eq!(
            u32::from_le_bytes(pod[12..16].try_into().unwrap()),
            SPA_PARAM_Format
        );
        // The pod advertises BGRx first; parse returns the default.
        let (width, height, layout) = parse_format_pod(&pod).expect("pod must parse");
        assert_eq!((width, height), (640, 360));
        assert_eq!(layout, PwPixelLayout::Bgra8);
    }

    #[test]
    fn parse_accepts_real_style_negotiated_pod() {
        // Plain-Id format, 1920x1080 — the shape PipeWire sends after
        // negotiation (not our own builder's Choice wrapper).
        let pod = real_negotiated_pod(SPA_VIDEO_FORMAT_BGRx, 1920, 1080);
        let (width, height, layout) = parse_format_pod(&pod).expect("real pod must parse");
        assert_eq!((width, height), (1920, 1080));
        assert_eq!(layout, PwPixelLayout::Bgra8);

        // A 24-bit negotiated format parses to the 24-bit layout.
        let pod = real_negotiated_pod(SPA_VIDEO_FORMAT_RGB, 1280, 720);
        let (_, _, layout) = parse_format_pod(&pod).unwrap();
        assert_eq!(layout, PwPixelLayout::Rgb24);
    }

    #[test]
    fn parse_rejects_garbage_and_unsupported_formats() {
        assert!(parse_format_pod(&[]).is_none());
        assert!(parse_format_pod(&[0, 0, 0, 0, 1, 0, 0, 0]).is_none()); // not an object
                                                                        // Object but truncated body.
        let mut truncated = vec![0u8; 16];
        truncated[4..8].copy_from_slice(&SPA_TYPE_Object.to_le_bytes());
        assert!(parse_format_pod(&truncated).is_none());
        // Real pod but an unsupported (YUV) format id.
        let yuv = real_negotiated_pod(2, 640, 480);
        assert!(parse_format_pod(&yuv).is_none());
    }

    #[test]
    fn parse_renegotiated_size_updates_geometry() {
        // The same pod shape with a different size models a display
        // resolution change: negotiation → renegotiation.
        let a = parse_format_pod(&real_negotiated_pod(SPA_VIDEO_FORMAT_BGRA, 1920, 1080)).unwrap();
        let b = parse_format_pod(&real_negotiated_pod(SPA_VIDEO_FORMAT_BGRA, 3840, 2160)).unwrap();
        assert_eq!((a.0, a.1), (1920, 1080));
        assert_eq!((b.0, b.1), (3840, 2160));
        assert_eq!(a.2, b.2);
    }

    #[test]
    fn normalize_4bpp_drops_row_padding() {
        // 2x2 BGRA, chunk stride 16 bytes (tight rows are 8 bytes, so each
        // row has 8 bytes of padding). The two rows must come out tight.
        let layout = PwPixelLayout::Bgra8;
        let mut src = vec![0u8; 32];
        // Row 0: pixels (B,G,R,A) = (1,2,3,255), (4,5,6,255) then 8 pad bytes.
        src[0..8].copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
        src[8..16].copy_from_slice(&[0xEE; 8]);
        // Row 1: (7,8,9,255), (10,11,12,255) then 8 pad bytes.
        src[16..24].copy_from_slice(&[7, 8, 9, 255, 10, 11, 12, 255]);
        src[24..32].copy_from_slice(&[0xEE; 8]);
        let out = normalize_buffer(&src, 0, 2, 2, layout, 16).unwrap();
        assert_eq!(out.len(), 16);
        assert_eq!(
            out,
            vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    #[test]
    fn normalize_honors_chunk_offset() {
        let layout = PwPixelLayout::Rgba8;
        let mut src = vec![0u8; 24];
        src[8..24].copy_from_slice(&[9, 9, 9, 255, 8, 8, 8, 255, 7, 7, 7, 255, 6, 6, 6, 255]);
        let out = normalize_buffer(&src, 8, 2, 2, layout, 0).unwrap();
        assert_eq!(
            out,
            vec![9, 9, 9, 255, 8, 8, 8, 255, 7, 7, 7, 255, 6, 6, 6, 255]
        );
    }

    #[test]
    fn normalize_expands_bgr24_to_bgra8() {
        // 2x1 BGR24: bytes are B,G,R per pixel.
        let src = [10, 20, 30, 40, 50, 60];
        let out = normalize_buffer(&src, 0, 2, 1, PwPixelLayout::Bgr24, 0).unwrap();
        assert_eq!(out, vec![10, 20, 30, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn normalize_expands_rgb24_to_rgba8_with_padding() {
        // 2x1 RGB24 with a padded stride of 8 bytes (row bytes = 6).
        let mut src = vec![0u8; 8];
        src[0..6].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        let out = normalize_buffer(&src, 0, 2, 1, PwPixelLayout::Rgb24, 8).unwrap();
        assert_eq!(out, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn normalize_rejects_short_buffer_and_small_stride() {
        let layout = PwPixelLayout::Bgra8;
        // Buffer shorter than stride * height.
        let src = vec![0u8; 8];
        assert!(normalize_buffer(&src, 0, 2, 2, layout, 16).is_err());
        // Stride smaller than tight row.
        assert!(normalize_buffer(&src, 0, 4, 1, layout, 8).is_err());
        // Offset past the buffer.
        assert!(normalize_buffer(&src, 9, 2, 1, layout, 0).is_err());
        // A 1x1 frame needs 4 bytes; an empty buffer cannot hold it.
        assert!(normalize_buffer(&[], 0, 1, 1, layout, 0).is_err());
        // Zero geometry normalizes to an empty buffer (the stream callback
        // guards width/height > 0 before calling, so this never feeds a
        // real frame).
        assert_eq!(
            normalize_buffer(&[], 0, 0, 0, layout, 0).unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn normalize_reports_stream_error_kind() {
        let err = normalize_buffer(&[0u8; 4], 0, 8, 8, PwPixelLayout::Bgra8, 0).unwrap_err();
        assert_eq!(
            err.kind(),
            crate::screen_share::ScreenShareErrorKind::Stream
        );
    }

    /// Build a synthetic `spa_meta_cursor` blob: header (id, flags,
    /// position, hotspot, bitmap_offset) plus an inline `spa_meta_bitmap`
    /// and ARGB8888 pixel data at the bitmap struct's `offset`.
    fn cursor_meta_blob(
        id: u32,
        flags: u32,
        x: i32,
        y: i32,
        bitmap: Option<(u32, u32, u32, u32, Vec<u8>)>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(&8i32.to_le_bytes()); // hotspot x (unused)
        bytes.extend_from_slice(&8i32.to_le_bytes()); // hotspot y (unused)
        match bitmap {
            Some((format, width, height, stride, pixels)) => {
                // bitmap struct begins immediately after the 28-byte header.
                let bitmap_offset = 28u32;
                bytes.extend_from_slice(&bitmap_offset.to_le_bytes());
                bytes.extend_from_slice(&format.to_le_bytes());
                bytes.extend_from_slice(&width.to_le_bytes());
                bytes.extend_from_slice(&height.to_le_bytes());
                bytes.extend_from_slice(&(stride as i32).to_le_bytes());
                // Pixel data offset relative to the bitmap struct start:
                // 28 (header) + 20 (bitmap struct) = 48.
                bytes.extend_from_slice(&20u32.to_le_bytes());
                bytes.extend_from_slice(&pixels);
            }
            None => {
                bytes.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn spa_cursor_meta_parses_position_and_bitmap() {
        // A 2x2 ARGB8888 bitmap: white opaque pixel + transparent pixel.
        let pixels = vec![
            255, 255, 255, 255, // [A,R,G,B] → white
            0, 255, 0, 255, // transparent red → BGRA [B,G,R,A] with A=0
            255, 0, 255, 0, // opaque green
            255, 0, 0, 255, // opaque blue → BGRA red
        ];
        let blob = cursor_meta_blob(
            7,
            0,
            123,
            456,
            Some((SPA_VIDEO_FORMAT_ARGB, 2, 2, 8, pixels)),
        );
        let parsed = parse_spa_cursor_meta(&blob).expect("parse");
        assert_eq!(parsed.x, 123);
        assert_eq!(parsed.y, 456);
        assert!(parsed.visible);
        let sprite = parsed.sprite.expect("sprite present");
        assert_eq!((sprite.width, sprite.height), (2, 2));
        // First pixel: ARGB (A=255,R=255,G=255,B=255) → BGRA white.
        assert_eq!(&sprite.pixels[0..4], &[255, 255, 255, 255]);
        // Third pixel: ARGB (A=255,R=0,G=255,B=0) → BGRA (0,255,0,255).
        assert_eq!(&sprite.pixels[8..12], &[0, 255, 0, 255]);
    }

    #[test]
    fn spa_cursor_meta_hide_flag_and_position_only() {
        // HIDE flag set → visible = false.
        let hidden = cursor_meta_blob(1, SPA_META_CURSOR_FLAG_HIDE, 10, 20, None);
        let parsed = parse_spa_cursor_meta(&hidden).expect("parse hidden");
        assert!(!parsed.visible);
        assert!(parsed.sprite.is_none(), "no bitmap → position-only");
        // id == 0 → no new cursor data.
        let none = cursor_meta_blob(0, 0, 10, 20, None);
        assert!(parse_spa_cursor_meta(&none).is_none());
        // Truncated blob → None.
        assert!(parse_spa_cursor_meta(&[0u8; 8]).is_none());
    }

    #[test]
    fn spa_cursor_meta_rejects_malformed_bitmap() {
        // Bitmap claims 200x200 (over the 128 cap) → whole blob rejected.
        let blob = cursor_meta_blob(
            1,
            0,
            0,
            0,
            Some((SPA_VIDEO_FORMAT_ARGB, 200, 200, 0, vec![])),
        );
        assert!(parse_spa_cursor_meta(&blob).is_none());
        // Bitmap with unknown format → rejected.
        let blob = cursor_meta_blob(1, 0, 0, 0, Some((99, 2, 2, 8, vec![0u8; 16])));
        assert!(parse_spa_cursor_meta(&blob).is_none());
    }

    #[test]
    fn spa_cursor_meta_accepts_bgra_bitmap_passthrough() {
        // BGRA8 (12) sprite passes through without channel swap.
        let pixels = vec![1u8, 2, 3, 255, 5, 6, 7, 255];
        let blob = cursor_meta_blob(2, 0, 0, 0, Some((SPA_VIDEO_FORMAT_BGRA, 2, 1, 8, pixels)));
        let parsed = parse_spa_cursor_meta(&blob).expect("parse bgra");
        assert_eq!(
            &parsed.sprite.as_ref().expect("sprite").pixels[0..4],
            &[1, 2, 3, 255]
        );
        assert_eq!(
            &parsed.sprite.as_ref().unwrap().pixels[4..8],
            &[5, 6, 7, 255]
        );
    }
}
