//! Pure, platform-independent coordinate normalization for screen sharing.
//!
//! # Cursor strategy (BORU-SS-12 / PDF Task 4.2)
//!
//! **Decision: the Windows cursor is composited into captured frames on the
//! host, not represented as a separate stream.**
//!
//! `Windows.Graphics.Capture` (the WinRT API used by the Windows backend in
//! [`crate::screen_share::platform::windows`]) deliberately does **not**
//! include the pointer in `Direct3D11CaptureFrame` surfaces. The host
//! therefore queries the cursor with GDI (`GetCursorInfo`/`GetIconInfo`),
//! rasterizes its shape, and alpha-blends it into the BGRA8 frame at the
//! source-relative position *before* the frame reaches the encoder.
//!
//! Rationale:
//! - The current pipeline (capture → BGRA8 CPU frame → OpenH264 → protocol →
//!   viewer) renders frames as-is, so a composited cursor appears on every
//!   viewer with **zero protocol or viewer changes**.
//! - A separate representation (cursor shape + position messages and
//!   viewer-side compositing) is listed as a *future* optimization in the
//!   reference PDF Phase 14 ("cursor-shape optimization"). Doing it now would
//!   require new protocol messages, viewer rendering, and cursor lifetime
//!   management — out of proportion for the baseline.
//! - Compositing cost is a per-frame alpha blend of a ~32×32 sprite; at the
//!   baseline 15 fps this is negligible. The main downside (cursor motion
//!   invalidates the frame region) is already paid by full-frame encoding.
//!
//! The pure mapping and blending below are compiled and unit-tested on every
//! target (including Linux CI); only the GDI cursor query lives in the
//! Windows backend.
//!
//! # Coordinate model
//!
//! Windows places every monitor in a single virtual desktop. The primary
//! monitor's top-left is `(0, 0)`; monitors to the left or above it have
//! **negative** origins. `GetMonitorInfo` returns each monitor's `rcMonitor`
//! in this virtual-desktop space (physical pixels), and
//! `GetCursorPos`/`GetCursorInfo` return the pointer in the same space.
//!
//! All Boru screen-share coordinates are normalized against the **shared
//! source** — the monitor being captured — never against the global desktop.
//! A cursor at desktop `(-960, 540)` on a monitor whose top-left is
//! `(-1920, 0)` is at source-relative `(960, 540)`, which is the same pixel
//! the capture backend reported in the frame. Keeping the source origin in
//! the geometry (rather than assuming the primary monitor is at `(0,0)`)
//! makes multi-monitor layouts with negative coordinates correct by
//! construction.
//!
//! DPI: Windows reports physical pixels for both monitor geometry and cursor
//! position when the process is per-monitor-DPI-aware (Boru requests that
//! awareness in the backend). The mapping functions therefore operate in
//! physical pixels and are DPI-independent. The `logical_to_physical` /
//! `physical_to_logical` helpers convert between logical (DIP) and physical
//! units for callers that only know logical sizes (e.g. portal backends), so
//! mixed-DPI and 100%–200% scaling layouts are representable.

/// A point in virtual-desktop coordinates (physical pixels). Can be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopPoint {
    /// Horizontal virtual-desktop coordinate (physical px; may be negative).
    pub x: i32,
    /// Vertical virtual-desktop coordinate (physical px; may be negative).
    pub y: i32,
}

/// A point relative to the shared source, in source pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePoint {
    /// Horizontal offset from the source's left edge.
    pub x: u32,
    /// Vertical offset from the source's top edge.
    pub y: u32,
}

/// A point normalized against the shared source, `0..=1` on both axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedPoint {
    /// Normalized horizontal position within the source (`0..=1`).
    pub x: f64,
    /// Normalized vertical position within the source (`0..=1`).
    pub y: f64,
}

/// Physical-pixel geometry of one monitor in the virtual desktop.
///
/// `left`/`top` are the monitor's origin in virtual-desktop coordinates and
/// may be negative (monitor left of / above the primary). `width`/`height`
/// are the monitor's physical pixel size — the same dimensions the capture
/// backend reports in frames, so mapping is direct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    /// Left edge of the monitor in virtual-desktop coordinates (physical px;
    /// negative for monitors left of the primary).
    pub left: i32,
    /// Top edge of the monitor in virtual-desktop coordinates (physical px;
    /// negative for monitors above the primary).
    pub top: i32,
    /// Physical pixel width of the monitor.
    pub width: u32,
    /// Physical pixel height of the monitor.
    pub height: u32,
}

impl MonitorGeometry {
    /// Construct a monitor geometry from its virtual-desktop origin and
    /// physical size.
    pub fn new(left: i32, top: i32, width: u32, height: u32) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    /// Whether the desktop point lies within this monitor's bounds
    /// (half-open: right/bottom edges are exclusive, matching pixel indexing).
    pub fn contains(&self, point: DesktopPoint) -> bool {
        point.x >= self.left
            && point.y >= self.top
            && point.x < self.left.saturating_add(self.width as i32)
            && point.y < self.top.saturating_add(self.height as i32)
    }
}

/// Map a virtual-desktop point to source-relative pixels.
///
/// Returns `None` when the point is outside the shared source. This is the
/// core normalization: it subtracts the source's desktop origin instead of
/// assuming the primary monitor is at `(0,0)`, so monitors with negative
/// coordinates (left of / above the primary) map correctly.
pub fn desktop_to_source(point: DesktopPoint, geometry: &MonitorGeometry) -> Option<SourcePoint> {
    if !geometry.contains(point) {
        return None;
    }
    Some(SourcePoint {
        x: (point.x - geometry.left) as u32,
        y: (point.y - geometry.top) as u32,
    })
}

/// Map source-relative pixels back to virtual-desktop coordinates by adding
/// the source's desktop origin.
pub fn source_to_desktop(point: SourcePoint, geometry: &MonitorGeometry) -> DesktopPoint {
    DesktopPoint {
        x: geometry.left.saturating_add(point.x as i32),
        y: geometry.top.saturating_add(point.y as i32),
    }
}

/// Map a virtual-desktop point to normalized source coordinates (`0..=1`).
///
/// Returns `None` when the point is outside the shared source. Normalized
/// coordinates are relative to the source, never to the global desktop, so a
/// viewer can address the shared image identically regardless of where the
/// source sits in the desktop layout.
pub fn desktop_to_normalized(
    point: DesktopPoint,
    geometry: &MonitorGeometry,
) -> Option<NormalizedPoint> {
    let source = desktop_to_source(point, geometry)?;
    Some(NormalizedPoint {
        x: source.x as f64 / geometry.width as f64,
        y: source.y as f64 / geometry.height as f64,
    })
}

/// Map normalized source coordinates to source-relative pixels.
///
/// Returns `None` when the normalized point lies at or beyond the right/bottom
/// edge (`1.0` is outside the last pixel, matching pixel indexing).
pub fn normalized_to_source(
    point: NormalizedPoint,
    geometry: &MonitorGeometry,
) -> Option<SourcePoint> {
    if !point.x.is_finite() || !point.y.is_finite() {
        return None;
    }
    let x = point.x * geometry.width as f64;
    let y = point.y * geometry.height as f64;
    if x < 0.0 || y < 0.0 || x >= geometry.width as f64 || y >= geometry.height as f64 {
        return None;
    }
    Some(SourcePoint {
        x: x.floor() as u32,
        y: y.floor() as u32,
    })
}

/// Map normalized source coordinates to virtual-desktop coordinates.
///
/// This is the inverse of [`desktop_to_normalized`]; it is used by remote
/// input paths that receive viewer-normalized points and must place them on
/// the actual desktop (e.g. SendInput injection).
pub fn normalized_to_desktop(
    point: NormalizedPoint,
    geometry: &MonitorGeometry,
) -> Option<DesktopPoint> {
    let source = normalized_to_source(point, geometry)?;
    Some(source_to_desktop(source, geometry))
}

/// Convert logical (DIP) pixels to physical pixels at a DPI scale factor
/// (1.0 = 100%, 1.25 = 125%, 1.5 = 150%, 2.0 = 200%).
pub fn logical_to_physical(value: f64, scale: f64) -> i32 {
    (value * scale).round() as i32
}

/// Convert physical pixels to logical (DIP) pixels at a DPI scale factor.
pub fn physical_to_logical(value: i32, scale: f64) -> f64 {
    value as f64 / scale
}

/// Build physical-pixel monitor geometry from a logical (DIP) rect and a DPI
/// scale factor.
///
/// Used by backends that enumerate monitors in logical units (e.g. portal
/// backends) so they can still produce physical geometry for the mapping
/// functions above.
pub fn geometry_from_logical(
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    scale: f64,
) -> MonitorGeometry {
    MonitorGeometry::new(
        logical_to_physical(left, scale),
        logical_to_physical(top, scale),
        logical_to_physical(width, scale).max(0) as u32,
        logical_to_physical(height, scale).max(0) as u32,
    )
}

/// A rasterized cursor shape ready to blend into a frame.
///
/// Pixels are BGRA8, `width * height * 4` bytes, top-down. Alpha `0` is fully
/// transparent; `255` is fully opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSprite {
    /// Sprite width in pixels.
    pub width: u32,
    /// Sprite height in pixels.
    pub height: u32,
    /// Hotspot offset from the sprite's top-left (the point that "is" the
    /// cursor, reported by the OS as the cursor position).
    pub hotspot_x: u32,
    /// Hotspot offset from the sprite's top edge.
    pub hotspot_y: u32,
    /// BGRA8 pixel payload, `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl CursorSprite {
    /// Construct a cursor sprite, validating its pixel buffer length.
    pub fn new(
        width: u32,
        height: u32,
        hotspot_x: u32,
        hotspot_y: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, crate::screen_share::ScreenShareError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| {
                crate::screen_share::ScreenShareError::new("cursor sprite dimensions overflow")
            })?;
        if pixels.len() != expected {
            return Err(crate::screen_share::ScreenShareError::new(
                "cursor sprite pixel buffer does not match width * height * 4",
            ));
        }
        Ok(Self {
            width,
            height,
            hotspot_x,
            hotspot_y,
            pixels,
        })
    }
}

/// Alpha-blend a cursor sprite into a BGRA8 frame at a desktop cursor
/// position.
///
/// `cursor_pos` is the pointer's hotspot in virtual-desktop coordinates; the
/// sprite is placed so its hotspot lands on that pixel, then clipped to the
/// frame. Returns `true` when at least one pixel was drawn.
///
/// This is the pure half of the Windows cursor strategy: the backend only
/// needs to rasterize the cursor shape, then call this with the shared
/// source's geometry. Everything here is unit-tested on Linux.
pub fn composite_cursor(
    frame: &mut [u8],
    frame_width: u32,
    frame_height: u32,
    cursor_pos: DesktopPoint,
    geometry: &MonitorGeometry,
    sprite: &CursorSprite,
) -> bool {
    let Some(source) = desktop_to_source(cursor_pos, geometry) else {
        return false;
    };
    let top = source.y as i64 - sprite.hotspot_y as i64;
    let left = source.x as i64 - sprite.hotspot_x as i64;
    let right = left + sprite.width as i64;
    let bottom = top + sprite.height as i64;

    let clip_left = left.max(0) as u32;
    let clip_top = top.max(0) as u32;
    let clip_right = right.min(frame_width as i64).max(0) as u32;
    let clip_bottom = bottom.min(frame_height as i64).max(0) as u32;
    if clip_left >= clip_right || clip_top >= clip_bottom {
        return false;
    }

    let mut drew = false;
    for fy in clip_top..clip_bottom {
        let sy = (fy as i64 - top) as u32;
        for fx in clip_left..clip_right {
            let sx = (fx as i64 - left) as u32;
            let s = ((sy * sprite.width + sx) * 4) as usize;
            let a = sprite.pixels[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = ((fy * frame_width + fx) * 4) as usize;
            let inv = 255 - a;
            frame[d] = ((sprite.pixels[s] as u32 * a + frame[d] as u32 * inv) / 255) as u8;
            frame[d + 1] =
                ((sprite.pixels[s + 1] as u32 * a + frame[d + 1] as u32 * inv) / 255) as u8;
            frame[d + 2] =
                ((sprite.pixels[s + 2] as u32 * a + frame[d + 2] as u32 * inv) / 255) as u8;
            frame[d + 3] = 255;
            drew = true;
        }
    }
    drew
}

/// Cursor shape+position metadata detached from the frame pixels (PDF Task
/// 5.3 `Metadata` cursor mode, PipeWire `spa_meta_cursor`, XFixes cursor
/// notify).
///
/// When a capture backend delivers this, the host sends shape-on-change and
/// position-per-move control messages instead of compositing the cursor into
/// the encoded frame, so cursor motion never forces a full-frame re-encode.
/// `sprite` is `Some` only when the shape changed since the previous frame
/// (position-only updates carry `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorMeta {
    /// Cursor position in virtual-desktop coordinates (physical pixels).
    pub position: DesktopPoint,
    /// Whether the cursor is visible at `position`.
    pub visible: bool,
    /// The current cursor sprite when the shape changed; `None` when the
    /// shape is unchanged since the previous frame.
    pub sprite: Option<CursorSprite>,
}

impl CursorMeta {
    /// Build a position-only update (no shape change).
    pub fn position(position: DesktopPoint, visible: bool) -> Self {
        Self {
            position,
            visible,
            sprite: None,
        }
    }

    /// Build a full shape+position update.
    pub fn with_sprite(position: DesktopPoint, visible: bool, sprite: CursorSprite) -> Self {
        Self {
            position,
            visible,
            sprite: Some(sprite),
        }
    }
}

/// Map a normalized cursor position to the top-left of the sprite's
/// displayed rect on the VIEWER surface, reusing the surface-geometry scale
/// (fit/100%/zoom/pan).
///
/// `surface_scale` is the scale the viewer surface applies to the source
/// image (screen px per source px — `SurfaceGeometry::scale()`), so the
/// sprite is drawn at the same scale as the image it overlays. The hotspot
/// lands exactly on the cursor position; the returned rect is in viewport
/// pixels. `None` when the cursor is fully outside the viewport.
pub fn cursor_viewport_rect(
    position: NormalizedPoint,
    source_size: (f32, f32),
    viewport_size: (f32, f32),
    surface_scale: f32,
    sprite: &CursorSprite,
) -> Option<(f32, f32, f32, f32)> {
    if !position.x.is_finite() || !position.y.is_finite() {
        return None;
    }
    // NormalizedPoint is f64 (the shared coordinate contract); the surface
    // geometry is f32, so cast once up front.
    let nx = position.x as f32;
    let ny = position.y as f32;
    let src_x = nx * source_size.0;
    let src_y = ny * source_size.1;
    // The source image is centered in the viewport (letterboxed); offset the
    // sprite by the same letterbox as the image.
    let img_w = source_size.0 * surface_scale;
    let img_h = source_size.1 * surface_scale;
    let ox = (viewport_size.0 - img_w) / 2.0;
    let oy = (viewport_size.1 - img_h) / 2.0;
    let left = ox + src_x * surface_scale - sprite.hotspot_x as f32 * surface_scale;
    let top = oy + src_y * surface_scale - sprite.hotspot_y as f32 * surface_scale;
    let w = sprite.width as f32 * surface_scale;
    let h = sprite.height as f32 * surface_scale;
    // Fully outside the viewport (right/bottom edges may be partially
    // clipped by the surface; that is the view's job).
    if left >= viewport_size.0 || top >= viewport_size.1 || left + w <= 0.0 || top + h <= 0.0 {
        return None;
    }
    Some((left, top, w, h))
}

/// Alpha-blend a BGRA8 cursor sprite into an RGBA8 frame (viewer-side
/// compositing) at a source-relative position with the hotspot at
/// `position`. The frame is RGBA8 because the H.264 decoder emits RGBA;
/// the sprite is BGRA8 (the shared [`CursorSprite`] format), so the red and
/// blue channels are swapped while blending.
///
/// Returns `true` when at least one pixel was drawn.
pub fn composite_cursor_rgba(
    frame: &mut [u8],
    frame_width: u32,
    frame_height: u32,
    position: SourcePoint,
    sprite: &CursorSprite,
) -> bool {
    let top = position.y as i64 - sprite.hotspot_y as i64;
    let left = position.x as i64 - sprite.hotspot_x as i64;
    let right = left + sprite.width as i64;
    let bottom = top + sprite.height as i64;

    let clip_left = left.max(0) as u32;
    let clip_top = top.max(0) as u32;
    let clip_right = right.min(frame_width as i64).max(0) as u32;
    let clip_bottom = bottom.min(frame_height as i64).max(0) as u32;
    if clip_left >= clip_right || clip_top >= clip_bottom {
        return false;
    }

    let mut drew = false;
    for fy in clip_top..clip_bottom {
        let sy = (fy as i64 - top) as u32;
        for fx in clip_left..clip_right {
            let sx = (fx as i64 - left) as u32;
            let s = ((sy * sprite.width + sx) * 4) as usize;
            let a = sprite.pixels[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = ((fy * frame_width + fx) * 4) as usize;
            let inv = 255 - a;
            // Frame is RGBA, sprite is BGRA: swap red and blue.
            frame[d] = ((sprite.pixels[s + 2] as u32 * a + frame[d] as u32 * inv) / 255) as u8;
            frame[d + 1] =
                ((sprite.pixels[s + 1] as u32 * a + frame[d + 1] as u32 * inv) / 255) as u8;
            frame[d + 2] = ((sprite.pixels[s] as u32 * a + frame[d + 2] as u32 * inv) / 255) as u8;
            frame[d + 3] = 255;
            drew = true;
        }
    }
    drew
}

/// Scale a BGRA8 cursor sprite from the source resolution to the ENCODE
/// resolution (BORU-SS-33 host side). Cursor sprites are tiny (<= 128x128);
/// the encode frame is often smaller than the source, so the sprite must be
/// scaled to match the frame the viewer composites into. Nearest-neighbour
/// is imperceptible at these sizes and much cheaper than bilinear.
pub fn scale_sprite_to(
    sprite: &CursorSprite,
    source_width: u32,
    source_height: u32,
    encode_width: u32,
    encode_height: u32,
) -> CursorSprite {
    if source_width == 0 || source_height == 0 || encode_width == 0 || encode_height == 0 {
        return sprite.clone();
    }
    if source_width == encode_width && source_height == encode_height {
        return sprite.clone();
    }
    let dst_w = (((sprite.width as u64 * encode_width as u64) + source_width as u64 / 2)
        / source_width as u64)
        .max(1) as u32;
    let dst_h = (((sprite.height as u64 * encode_height as u64) + source_height as u64 / 2)
        / source_height as u64)
        .max(1) as u32;
    let mut pixels = vec![0u8; (dst_w * dst_h * 4) as usize];
    for y in 0..dst_h {
        let sy = (y as u64 * sprite.height as u64 / dst_h as u64) as u32;
        for x in 0..dst_w {
            let sx = (x as u64 * sprite.width as u64 / dst_w as u64) as u32;
            let from = ((sy * sprite.width + sx) * 4) as usize;
            let to = ((y * dst_w + x) * 4) as usize;
            pixels[to..to + 4].copy_from_slice(&sprite.pixels[from..from + 4]);
        }
    }
    CursorSprite {
        width: dst_w,
        height: dst_h,
        hotspot_x: (sprite.hotspot_x as u64 * dst_w as u64 / sprite.width as u64) as u32,
        hotspot_y: (sprite.hotspot_y as u64 * dst_h as u64 / sprite.height as u64) as u32,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid BGRA8 sprite with a white opaque body and a transparent border.
    fn arrow_sprite(width: u32, height: u32) -> CursorSprite {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let i = ((y * width + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        CursorSprite::new(width, height, width / 2, height / 2, pixels).unwrap()
    }

    #[test]
    fn primary_monitor_origin_is_identity() {
        let g = MonitorGeometry::new(0, 0, 1920, 1080);
        assert_eq!(
            desktop_to_source(DesktopPoint { x: 100, y: 50 }, &g),
            Some(SourcePoint { x: 100, y: 50 })
        );
        // Right/bottom edges are exclusive.
        assert_eq!(desktop_to_source(DesktopPoint { x: 1920, y: 0 }, &g), None);
        assert_eq!(desktop_to_source(DesktopPoint { x: 0, y: 1080 }, &g), None);
        // Negative desktop coords are outside a primary-at-origin monitor.
        assert_eq!(desktop_to_source(DesktopPoint { x: -1, y: 0 }, &g), None);
    }

    #[test]
    fn monitor_left_of_primary_uses_negative_origin() {
        // 1920x1080 monitor to the LEFT of the primary: origin (-1920, 0).
        let g = MonitorGeometry::new(-1920, 0, 1920, 1080);
        // Center of that monitor in desktop coords is (-960, 540).
        assert_eq!(
            desktop_to_source(DesktopPoint { x: -960, y: 540 }, &g),
            Some(SourcePoint { x: 960, y: 540 })
        );
        // Its top-left corner.
        assert_eq!(
            desktop_to_source(DesktopPoint { x: -1920, y: 0 }, &g),
            Some(SourcePoint { x: 0, y: 0 })
        );
        // The primary monitor's origin is NOT part of this source.
        assert_eq!(desktop_to_source(DesktopPoint { x: 0, y: 0 }, &g), None);
        // Just left of this monitor is outside.
        assert_eq!(desktop_to_source(DesktopPoint { x: -1921, y: 0 }, &g), None);
    }

    #[test]
    fn monitor_above_primary_uses_negative_top() {
        // 1920x1080 monitor ABOVE the primary: origin (0, -1080).
        let g = MonitorGeometry::new(0, -1080, 1920, 1080);
        assert_eq!(
            desktop_to_source(DesktopPoint { x: 100, y: -540 }, &g),
            Some(SourcePoint { x: 100, y: 540 })
        );
        assert_eq!(desktop_to_source(DesktopPoint { x: 100, y: 0 }, &g), None);
    }

    #[test]
    fn source_to_desktop_round_trips_through_negative_origin() {
        let g = MonitorGeometry::new(-1920, -120, 1920, 1080);
        let source = SourcePoint { x: 960, y: 540 };
        let desktop = source_to_desktop(source, &g);
        assert_eq!(desktop, DesktopPoint { x: -960, y: 420 });
        assert_eq!(desktop_to_source(desktop, &g), Some(source));
    }

    #[test]
    fn desktop_to_normalized_is_source_relative() {
        let g = MonitorGeometry::new(-1920, 0, 1920, 1080);
        assert_eq!(
            desktop_to_normalized(DesktopPoint { x: -960, y: 540 }, &g),
            Some(NormalizedPoint { x: 0.5, y: 0.5 })
        );
        assert_eq!(
            desktop_to_normalized(DesktopPoint { x: -1920, y: 0 }, &g),
            Some(NormalizedPoint { x: 0.0, y: 0.0 })
        );
        // A point on the primary desktop is outside this source.
        assert_eq!(desktop_to_normalized(DesktopPoint { x: 0, y: 0 }, &g), None);
    }

    #[test]
    fn normalized_to_desktop_round_trips_on_negative_origin() {
        let g = MonitorGeometry::new(-1920, 0, 1920, 1080);
        assert_eq!(
            normalized_to_desktop(NormalizedPoint { x: 0.5, y: 0.5 }, &g),
            Some(DesktopPoint { x: -960, y: 540 })
        );
        assert_eq!(
            normalized_to_desktop(NormalizedPoint { x: 0.25, y: 0.25 }, &g),
            Some(DesktopPoint { x: -1440, y: 270 })
        );
        // 1.0 is past the last pixel.
        assert_eq!(
            normalized_to_desktop(NormalizedPoint { x: 1.0, y: 0.5 }, &g),
            None
        );
        assert_eq!(
            normalized_to_desktop(
                NormalizedPoint {
                    x: f64::NAN,
                    y: 0.5
                },
                &g
            ),
            None
        );
    }

    #[test]
    fn normalized_to_source_maps_inside_bounds() {
        let g = MonitorGeometry::new(0, 0, 1920, 1080);
        assert_eq!(
            normalized_to_source(NormalizedPoint { x: 0.25, y: 0.25 }, &g),
            Some(SourcePoint { x: 480, y: 270 })
        );
        assert_eq!(
            normalized_to_source(NormalizedPoint { x: 0.0, y: 0.0 }, &g),
            Some(SourcePoint { x: 0, y: 0 })
        );
        assert_eq!(
            normalized_to_source(NormalizedPoint { x: -0.1, y: 0.5 }, &g),
            None
        );
    }

    #[test]
    fn logical_physical_conversions_cover_scaling_percentages() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            // 100 logical px at this scale.
            let physical = logical_to_physical(100.0, scale);
            assert_eq!(physical, (100.0 * scale).round() as i32);
            // Round trip back.
            assert_eq!(physical_to_logical(physical, scale), 100.0);
        }
        // Concrete 150% example: 1280 logical px -> 1920 physical.
        assert_eq!(logical_to_physical(1280.0, 1.5), 1920);
        assert_eq!(logical_to_physical(720.0, 1.5), 1080);
        // 200% example.
        assert_eq!(logical_to_physical(960.0, 2.0), 1920);
    }

    #[test]
    fn mixed_dpi_layout_builds_physical_geometry_from_logical() {
        // Primary at 100%: logical = physical = 1920x1080 at (0,0).
        let primary = MonitorGeometry::new(0, 0, 1920, 1080);
        // Secondary to the right at 150%: logical 1280x720 at 150% ->
        // physical 1920x1080. geometry_from_logical scales origin AND size
        // uniformly at the monitor's DPI (a pure unit conversion), so the
        // physical origin is (2880, 0).
        let secondary = geometry_from_logical(1920.0, 0.0, 1280.0, 720.0, 1.5);
        assert_eq!(secondary, MonitorGeometry::new(2880, 0, 1920, 1080));
        // A logical desktop point at the secondary's center:
        // logical (1920 + 640, 360) -> physical (3840, 540).
        let cursor = DesktopPoint {
            x: logical_to_physical(1920.0 + 640.0, 1.5),
            y: logical_to_physical(360.0, 1.5),
        };
        assert_eq!(cursor, DesktopPoint { x: 3840, y: 540 });
        // The center of the shared source maps to the center: (960, 540).
        assert_eq!(
            desktop_to_source(cursor, &secondary),
            Some(SourcePoint { x: 960, y: 540 })
        );
        // The same physical point is outside the primary (which ends at
        // x=1920), so mixed-DPI sources are unambiguous.
        assert_eq!(desktop_to_source(cursor, &primary), None);
    }

    #[test]
    fn composite_cursor_blends_at_hotspot() {
        let mut frame = vec![0u8; 100 * 100 * 4]; // black BGRA8
        let sprite = arrow_sprite(10, 10);
        let g = MonitorGeometry::new(0, 0, 100, 100);
        // Cursor hotspot at desktop (50, 50): sprite top-left at (45, 45).
        let drew = composite_cursor(
            &mut frame,
            100,
            100,
            DesktopPoint { x: 50, y: 50 },
            &g,
            &sprite,
        );
        assert!(drew);
        // Center pixel (50,50) is opaque white after blend.
        let i = ((50 * 100 + 50) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[255, 255, 255, 255]);
        // Transparent border pixel (45,45) stays black.
        let i = ((45 * 100 + 45) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[0, 0, 0, 0]);
        // A pixel outside the sprite is untouched.
        let i = ((10 * 100 + 10) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn composite_cursor_clips_at_frame_edges() {
        let mut frame = vec![0u8; 100 * 100 * 4];
        let sprite = arrow_sprite(10, 10);
        let g = MonitorGeometry::new(0, 0, 100, 100);
        // Cursor at (3, 3): most of the sprite hangs off the top-left.
        let drew = composite_cursor(
            &mut frame,
            100,
            100,
            DesktopPoint { x: 3, y: 3 },
            &g,
            &sprite,
        );
        assert!(drew);
        // The visible pixel at the hotspot is white.
        let i = ((3 * 100 + 3) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn composite_cursor_outside_source_is_noop() {
        let mut frame = vec![0u8; 100 * 100 * 4];
        let sprite = arrow_sprite(10, 10);
        let g = MonitorGeometry::new(0, 0, 100, 100);
        // Cursor on the primary desktop (negative origin monitor) -> outside.
        let drew = composite_cursor(
            &mut frame,
            100,
            100,
            DesktopPoint { x: -100, y: 0 },
            &g,
            &sprite,
        );
        assert!(!drew);
        assert!(frame.iter().all(|&b| b == 0));
    }

    #[test]
    fn composite_cursor_on_negative_origin_monitor() {
        let mut frame = vec![0u8; 100 * 100 * 4];
        let sprite = arrow_sprite(10, 10);
        // Shared source is the monitor LEFT of the primary.
        let g = MonitorGeometry::new(-1920, 0, 100, 100);
        let drew = composite_cursor(
            &mut frame,
            100,
            100,
            DesktopPoint {
                x: -1920 + 50,
                y: 50,
            },
            &g,
            &sprite,
        );
        assert!(drew);
        let i = ((50 * 100 + 50) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn cursor_sprite_validates_pixel_length() {
        assert!(CursorSprite::new(2, 2, 1, 1, vec![0u8; 16]).is_ok());
        assert!(CursorSprite::new(2, 2, 1, 1, vec![0u8; 15]).is_err());
        assert!(CursorSprite::new(2, 2, 1, 1, vec![0u8; 17]).is_err());
    }

    #[test]
    fn geometry_contains_is_half_open() {
        let g = MonitorGeometry::new(-10, -10, 10, 10);
        assert!(g.contains(DesktopPoint { x: -10, y: -10 }));
        assert!(g.contains(DesktopPoint { x: -1, y: -1 }));
        assert!(!g.contains(DesktopPoint { x: 0, y: 0 }));
        assert!(!g.contains(DesktopPoint { x: -11, y: 0 }));
    }

    #[test]
    fn cursor_meta_constructors_preserve_shape_flag() {
        let pos = DesktopPoint { x: 10, y: 20 };
        let pos_only = CursorMeta::position(pos, true);
        assert!(pos_only.sprite.is_none());
        assert_eq!(pos_only.position, pos);
        assert!(pos_only.visible);
        let sprite = arrow_sprite(4, 4);
        let full = CursorMeta::with_sprite(pos, false, sprite.clone());
        assert_eq!(full.sprite.as_ref(), Some(&sprite));
        assert!(!full.visible);
    }

    #[test]
    fn cursor_viewport_rect_places_hotspot_on_position() {
        let sprite = arrow_sprite(10, 10); // hotspot (5, 5)
                                           // 640x360 source at 2x scale in a 1280x720 viewport: no letterbox.
        let rect = cursor_viewport_rect(
            NormalizedPoint { x: 0.5, y: 0.5 },
            (640.0, 360.0),
            (1280.0, 720.0),
            2.0,
            &sprite,
        )
        .unwrap();
        // Source center (320, 180) * 2 = (640, 360); hotspot 5*2=10 offset.
        assert!((rect.0 - 630.0).abs() < 1e-3, "left = {}", rect.0);
        assert!((rect.1 - 350.0).abs() < 1e-3, "top = {}", rect.1);
        assert!((rect.2 - 20.0).abs() < 1e-3);
        assert!((rect.3 - 20.0).abs() < 1e-3);
    }

    #[test]
    fn cursor_viewport_rect_fit_mode_letterboxes_and_scales() {
        let sprite = arrow_sprite(10, 10);
        // 16:9 source in a 4:3 viewport at fit: scale limited by height.
        let viewport = (800.0, 600.0);
        let scale: f32 = (800.0f32 / 640.0).min(600.0f32 / 360.0); // 1.25
        let rect = cursor_viewport_rect(
            NormalizedPoint { x: 0.0, y: 0.0 },
            (640.0, 360.0),
            viewport,
            scale,
            &sprite,
        )
        .unwrap();
        // Top-left of the source is letterboxed vertically:
        // ox = (800 - 640*1.25)/2 = 0, oy = (600 - 360*1.25)/2 = 75.
        // Hotspot (5,5) shifts the sprite up-left by 5*1.25.
        let left = 0.0 + 0.0 - 5.0 * 1.25;
        let top = 75.0 + 0.0 - 5.0 * 1.25;
        assert!((rect.0 - left).abs() < 1e-3, "left = {}", rect.0);
        assert!((rect.1 - top).abs() < 1e-3, "top = {}", rect.1);
        assert!((rect.2 - 12.5).abs() < 1e-3);
        assert!((rect.3 - 12.5).abs() < 1e-3);
    }

    #[test]
    fn cursor_viewport_rect_outside_viewport_is_none() {
        let sprite = arrow_sprite(10, 10);
        // Cursor far beyond the right edge of the viewport.
        assert!(cursor_viewport_rect(
            NormalizedPoint { x: 5.0, y: 0.5 },
            (640.0, 360.0),
            (1280.0, 720.0),
            2.0,
            &sprite,
        )
        .is_none());
        // Non-finite position is rejected.
        assert!(cursor_viewport_rect(
            NormalizedPoint {
                x: f64::NAN,
                y: 0.5
            },
            (640.0, 360.0),
            (1280.0, 720.0),
            2.0,
            &sprite,
        )
        .is_none());
    }

    #[test]
    fn composite_cursor_rgba_blends_into_rgba_frame_at_hotspot() {
        let mut frame = vec![0u8; 100 * 100 * 4]; // black RGBA8
        let sprite = arrow_sprite(10, 10); // white opaque body, BGRA8
        let drew =
            composite_cursor_rgba(&mut frame, 100, 100, SourcePoint { x: 50, y: 50 }, &sprite);
        assert!(drew);
        // Center pixel (50,50) is opaque white in RGBA.
        let i = ((50 * 100 + 50) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[255, 255, 255, 255]);
        // Transparent border pixel stays black.
        let i = ((45 * 100 + 45) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn composite_cursor_rgba_swaps_channels_correctly() {
        // A solid cyan sprite (BGRA: B=255,G=255,R=0) must land as
        // (R=0,G=255,B=255) in the RGBA frame.
        let mut frame = vec![0u8; 4 * 4 * 4];
        let sprite = CursorSprite::new(2, 2, 0, 0, vec![255, 255, 0, 255].repeat(4)).unwrap();
        let drew = composite_cursor_rgba(&mut frame, 4, 4, SourcePoint { x: 0, y: 0 }, &sprite);
        assert!(drew);
        let i = ((0 * 4 + 0) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[0, 255, 255, 255]);
    }

    #[test]
    fn composite_cursor_rgba_clips_at_frame_edges() {
        let mut frame = vec![0u8; 100 * 100 * 4];
        let sprite = arrow_sprite(10, 10);
        // Hotspot at (3,3): most of the sprite hangs off the top-left.
        let drew = composite_cursor_rgba(&mut frame, 100, 100, SourcePoint { x: 3, y: 3 }, &sprite);
        assert!(drew);
        let i = ((3 * 100 + 3) * 4) as usize;
        assert_eq!(&frame[i..i + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn composite_cursor_rgba_outside_frame_is_noop() {
        let mut frame = vec![0u8; 100 * 100 * 4];
        let sprite = arrow_sprite(10, 10);
        let drew = composite_cursor_rgba(
            &mut frame,
            100,
            100,
            SourcePoint { x: 500, y: 500 },
            &sprite,
        );
        assert!(!drew);
        assert!(frame.iter().all(|&b| b == 0));
    }

    #[test]
    fn scale_sprite_to_downscales_and_scales_hotspot() {
        // 32x32 sprite with hotspot (16,16) at 1920x1080 source scaled to
        // 640x360 encode: sprite becomes ~11x11, hotspot (5,5).
        let sprite = CursorSprite::new(32, 32, 16, 16, vec![255u8; 32 * 32 * 4]).unwrap();
        let scaled = scale_sprite_to(&sprite, 1920, 1080, 640, 360);
        assert_eq!(scaled.width, 11);
        assert_eq!(scaled.height, 11);
        assert_eq!(scaled.hotspot_x, 5);
        assert_eq!(scaled.hotspot_y, 5);
        assert_eq!(scaled.pixels.len(), 11 * 11 * 4);
    }

    #[test]
    fn scale_sprite_to_identity_when_resolutions_match() {
        let sprite = arrow_sprite(10, 10);
        let scaled = scale_sprite_to(&sprite, 640, 360, 640, 360);
        assert_eq!(scaled, sprite);
    }

    #[test]
    fn scale_sprite_to_zero_source_returns_original() {
        let sprite = arrow_sprite(10, 10);
        let scaled = scale_sprite_to(&sprite, 0, 0, 640, 360);
        assert_eq!(scaled, sprite);
    }
}
