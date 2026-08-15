//! Scalable screen-share surface (PDF Task 8.2).
//!
//! Renders decoded shared frames in a dedicated surface that preserves the
//! source aspect ratio and supports fit-to-window, 100% (actual pixels),
//! fullscreen, and optional pan/zoom. The geometry is pure and unit-tested;
//! the view builders wire it into iced `Image` + `mouse_area` widgets.
//!
//! Design notes:
//! - The surface element fills its allocated box. When the displayed image
//!   fits inside that box it is centered with no crop; when it overflows
//!   (zoomed in, or a source larger than the box at 100%), the visible
//!   sub-region is expressed as an `Image::crop` rect in source pixels so
//!   pan is just moving the crop window. Aspect ratio is preserved by
//!   `ContentFit::Contain` in both cases.
//! - Pointer-control input (BORU-SS-17) maps through the same geometry:
//!   a viewport point becomes a normalized source point, so remote input
//!   stays correct under pan/zoom instead of assuming a fixed 640x360 box.
//! - Zoom is anchored at the cursor when the wheel is used: the source
//!   point under the pointer stays under the pointer.

use super::*;

/// Presentation mode for the scalable surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScreenShareViewMode {
    /// Scale the whole source to fit the available box (aspect preserved).
    Fit,
    /// 1 source pixel = 1 screen pixel (actual size).
    Actual,
    /// Explicit scale factor relative to actual size (`1.0` == 100%).
    Zoom(f32),
}

impl Default for ScreenShareViewMode {
    fn default() -> Self {
        Self::Fit
    }
}

impl ScreenShareViewMode {
    /// Clamp a raw zoom factor to a sane viewing range.
    pub fn clamp_zoom(z: f32) -> f32 {
        z.clamp(0.05, 32.0)
    }
}

/// Pure geometry for the scalable surface.
///
/// All values are in f32 pixels. `pan` is the center of the visible region
/// in source pixels (`None` = source center).
#[derive(Debug, Clone, Copy)]
pub struct SurfaceGeometry {
    pub viewport: iced::Size,
    pub source: iced::Size,
    pub mode: ScreenShareViewMode,
    pub pan: Option<(f32, f32)>,
}

impl SurfaceGeometry {
    pub fn new(
        viewport: iced::Size,
        source: iced::Size,
        mode: ScreenShareViewMode,
        pan: Option<(f32, f32)>,
    ) -> Self {
        Self {
            viewport,
            source,
            mode,
            pan,
        }
    }

    /// Scale that fits the whole source into the viewport.
    pub fn fit_scale(&self) -> f32 {
        if self.source.width <= 0.0 || self.source.height <= 0.0 {
            return 1.0;
        }
        (self.viewport.width / self.source.width)
            .min(self.viewport.height / self.source.height)
            .max(0.0)
    }

    /// Current effective scale (screen px per source px).
    pub fn scale(&self) -> f32 {
        match self.mode {
            ScreenShareViewMode::Fit => self.fit_scale(),
            ScreenShareViewMode::Actual => 1.0,
            ScreenShareViewMode::Zoom(z) => ScreenShareViewMode::clamp_zoom(z),
        }
    }

    /// Displayed size of the whole source at the current scale.
    pub fn display_size(&self) -> iced::Size {
        let s = self.scale();
        iced::Size::new(self.source.width * s, self.source.height * s)
    }

    /// Whether the whole source is visible without panning.
    pub fn fits(&self) -> bool {
        let d = self.display_size();
        d.width <= self.viewport.width + 0.5 && d.height <= self.viewport.height + 0.5
    }

    /// Pan center in source pixels (clamped to source bounds).
    pub fn pan_center(&self) -> (f32, f32) {
        let (cx, cy) = self
            .pan
            .unwrap_or((self.source.width / 2.0, self.source.height / 2.0));
        (
            cx.clamp(0.0, self.source.width.max(0.0)),
            cy.clamp(0.0, self.source.height.max(0.0)),
        )
    }

    /// Visible region of the source, in source pixels `(x, y, w, h)`.
    ///
    /// The region is the part of the source mapped onto the viewport at the
    /// current scale, centered on the pan point and clamped to the source.
    pub fn visible_region(&self) -> (f32, f32, f32, f32) {
        let s = self.scale();
        if s <= 0.0 {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let vw = self.viewport.width / s;
        let vh = self.viewport.height / s;
        let w = vw.min(self.source.width);
        let h = vh.min(self.source.height);
        let (cx, cy) = self.pan_center();
        let x = (cx - w / 2.0).clamp(0.0, (self.source.width - w).max(0.0));
        let y = (cy - h / 2.0).clamp(0.0, (self.source.height - h).max(0.0));
        (x, y, w, h)
    }

    /// Rectangle in viewport coordinates where the image is drawn.
    ///
    /// When the image fits, this is the centered display rect. When it
    /// overflows (crop path), the image fills the viewport.
    pub fn display_rect(&self) -> iced::Rectangle {
        if self.fits() {
            let d = self.display_size();
            iced::Rectangle::new(
                iced::Point::new(
                    (self.viewport.width - d.width) / 2.0,
                    (self.viewport.height - d.height) / 2.0,
                ),
                d,
            )
        } else {
            iced::Rectangle::new(iced::Point::new(0.0, 0.0), self.viewport)
        }
    }

    /// Map a viewport point to a source pixel point (clamped to the source).
    pub fn viewport_to_source(&self, p: iced::Point) -> (f32, f32) {
        let s = self.scale();
        if s <= 0.0 {
            return (0.0, 0.0);
        }
        let r = self.display_rect();
        let (ox, oy, _, _) = self.visible_region();
        let sx = ox + (p.x - r.x) / s;
        let sy = oy + (p.y - r.y) / s;
        (
            sx.clamp(0.0, self.source.width),
            sy.clamp(0.0, self.source.height),
        )
    }

    /// Map a viewport point to normalized source coordinates (0..1), the
    /// contract used by the remote-control input path (BORU-SS-17).
    pub fn viewport_to_normalized(&self, p: iced::Point) -> (f32, f32) {
        let (sx, sy) = self.viewport_to_source(p);
        let w = self.source.width.max(1.0);
        let h = self.source.height.max(1.0);
        ((sx / w).clamp(0.0, 1.0), (sy / h).clamp(0.0, 1.0))
    }

    /// Pan center that keeps the source point under `anchor` fixed when the
    /// scale changes from the current geometry scale to `new_scale`.
    pub fn pan_for_zoom(&self, anchor: iced::Point, new_scale: f32) -> (f32, f32) {
        let (sx, sy) = self.viewport_to_source(anchor);
        let nx = sx - (anchor.x - self.viewport.width / 2.0) / new_scale;
        let ny = sy - (anchor.y - self.viewport.height / 2.0) / new_scale;
        (
            nx.clamp(0.0, self.source.width.max(0.0)),
            ny.clamp(0.0, self.source.height.max(0.0)),
        )
    }
}

/// Build the interactive scalable surface.
///
/// `hover` is the last known cursor position over the surface (used as the
/// wheel-zoom anchor); `control_active` switches the mouse area between
/// remote-control forwarding (BORU-SS-17) and local pan/zoom.
/// `last_pointer_norm` is the last sent normalized pointer position
/// (`screen_share_last_pointer_pos`), used for press/release coordinates in
/// control mode so clicks land where the cursor is (the old fixed-box path
/// used the same value).
pub(crate) fn view_screen_share_surface<'a>(
    handle: &'a iced::widget::image::Handle,
    source: iced::Size,
    viewport: iced::Size,
    mode: ScreenShareViewMode,
    pan: Option<(f32, f32)>,
    control_active: bool,
    hover: Option<iced::Point>,
    last_pointer_norm: Option<(f32, f32)>,
) -> iced::Element<'a, AppMessage> {
    use iced::widget::{container, image, mouse_area, text};
    use iced::Length;

    let geom = SurfaceGeometry::new(viewport, source, mode, pan);
    let scale = geom.scale();

    let image_el: iced::Element<'a, AppMessage> = if geom.fits() {
        let d = geom.display_size();
        image(handle.clone())
            .width(Length::Fixed(d.width))
            .height(Length::Fixed(d.height))
            .content_fit(iced::ContentFit::Contain)
            .into()
    } else {
        let (x, y, w, h) = geom.visible_region();
        image(handle.clone())
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(iced::ContentFit::Contain)
            .crop(iced::Rectangle {
                x: x.max(0.0) as u32,
                y: y.max(0.0) as u32,
                width: w.max(1.0) as u32,
                height: h.max(1.0) as u32,
            })
            .into()
    };

    let surface = container(image_el)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center);

    let surface_el: iced::Element<'a, AppMessage> = if control_active {
        // Remote control: forward pointer motion/buttons using the same
        // geometry, so normalized coordinates stay correct under pan/zoom.
        // Press/release use the last sent normalized position (updated on
        // every move) so clicks land where the cursor is.
        let g = geom;
        let (lx, ly) = last_pointer_norm.unwrap_or((0.0, 0.0));
        mouse_area(surface)
            .on_move(move |pos| {
                let (x, y) = g.viewport_to_normalized(pos);
                AppMessage::ScreenSharePointerMove { x, y }
            })
            .on_press(AppMessage::ScreenSharePointerButton {
                x: lx,
                y: ly,
                button: 1,
                pressed: true,
            })
            .on_release(AppMessage::ScreenSharePointerButton {
                x: lx,
                y: ly,
                button: 1,
                pressed: false,
            })
            // Wheel is a first-class remote input event (PDF Task 9.2): the
            // tick is forwarded with the last known pointer position so the
            // host scrolls where the viewer's cursor is.
            .on_scroll(move |delta| {
                let (dx, dy) = match delta {
                    iced::mouse::ScrollDelta::Lines { x, y } => (x, y),
                    iced::mouse::ScrollDelta::Pixels { x, y } => (x, y),
                };
                AppMessage::ScreenShareWheel { x: lx, y: ly, dx, dy }
            })
            .into()
    } else {
        // Local pan/zoom. Drag pans; wheel zooms around the cursor.
        let g = geom;
        // on_press takes a plain Message (not a closure), so the drag start
        // uses the last hover position — the same approximation the remote-
        // control path uses for press/release coordinates.
        let press_pos = hover.unwrap_or(iced::Point::new(
            viewport.width / 2.0,
            viewport.height / 2.0,
        ));
        let anchor = press_pos;
        let base_scale = scale;
        mouse_area(surface)
            .on_press(AppMessage::ScreenSharePanStart { pos: press_pos })
            .on_move(move |pos| AppMessage::ScreenSharePanMove {
                pos,
                scale: base_scale,
            })
            .on_release(AppMessage::ScreenSharePanEnd)
            .on_scroll(move |delta| {
                let factor = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => 1.1f32.powf(y),
                    iced::mouse::ScrollDelta::Pixels { y, .. } => (1.0 + y / 60.0).clamp(0.5, 2.0),
                };
                let old_scale = g.scale();
                let new_scale = ScreenShareViewMode::clamp_zoom(old_scale * factor);
                let new_pan = g.pan_for_zoom(anchor, new_scale);
                AppMessage::ScreenShareSetView {
                    mode: ScreenShareViewMode::Zoom(new_scale),
                    pan: Some(new_pan),
                }
            })
            .into()
    };

    if control_active {
        // Persistent visual indicator (PDF Task 9.1): a compact badge pinned
        // to the top-right of the shared surface while remote control is
        // active. The overlay container is not interactive — mouse events
        // fall through to the surface, so control input keeps working under
        // the indicator.
        let badge = container(
            text(crate::i18n::t("screenshare.control_badge"))
                .size(10)
                .color(iced::Color::WHITE),
        )
        .padding([3, 8])
        .style(|_| iced::widget::container::Style {
            background: Some(iced::Background::Color(
                iced::Color::from_rgb8(0xB3, 0x26, 0x1E),
            )),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let overlay = container(badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .padding(6);
        iced::widget::stack![surface_el, overlay].into()
    } else {
        surface_el
    }
}

/// Compact view-mode control row for the surface.
///
/// `scale` is the current effective scale (computed by the caller from the
/// same geometry the surface used) so the +/- buttons zoom relative to what
/// the user currently sees, including fit mode.
pub(crate) fn view_screen_share_view_controls<'a>(
    scale: f32,
    fullscreen: bool,
) -> iced::Element<'a, AppMessage> {
    use iced::widget::{button, row, text};

    let zoom_in = ScreenShareViewMode::clamp_zoom(scale * 1.25);
    let zoom_out = ScreenShareViewMode::clamp_zoom(scale / 1.25);

    row![
        button(text(crate::i18n::t("screenshare.fit")))
            .on_press(AppMessage::ScreenShareSetView {
                mode: ScreenShareViewMode::Fit,
                pan: None,
            })
            .padding([2, 6]),
        button(text(crate::i18n::t("screenshare.actual")))
            .on_press(AppMessage::ScreenShareSetView {
                mode: ScreenShareViewMode::Actual,
                pan: None,
            })
            .padding([2, 6]),
        button(text("−"))
            .on_press(AppMessage::ScreenShareSetView {
                mode: ScreenShareViewMode::Zoom(zoom_out),
                pan: None,
            })
            .padding([2, 6]),
        button(text("+"))
            .on_press(AppMessage::ScreenShareSetView {
                mode: ScreenShareViewMode::Zoom(zoom_in),
                pan: None,
            })
            .padding([2, 6]),
        button(text(crate::i18n::t("screenshare.reset_view")))
            .on_press(AppMessage::ScreenShareSetView {
                mode: ScreenShareViewMode::Fit,
                pan: None,
            })
            .padding([2, 6]),
        button(text(if fullscreen {
            crate::i18n::t("screenshare.inline")
        } else {
            crate::i18n::t("screenshare.fullscreen")
        }))
        .on_press(AppMessage::ToggleScreenShareFullscreen)
        .padding([2, 6]),
    ]
    .spacing(SPACE_6)
    .into()
}

/// The eight developer metrics for the diagnostics overlay (PDF Phase 12):
/// capture FPS, encode FPS, average encode time, bytes/sec, dropped frames,
/// queue depth, decode FPS, and estimated end-to-end latency — plus the
/// negotiated codec/dimensions/bitrate/backend line.
///
/// Rendered as compact lines so the same helper serves the host panel and the
/// viewer overlay. Contains no media data (never screen contents, raw frame
/// bytes, clipboard text, or keystrokes).
pub(crate) fn screen_share_metrics_lines(metrics: &ScreenShareSessionMetrics) -> Vec<String> {
    let s = metrics.snapshot;
    vec![
        format!(
            "{} · {}x{} @ {}fps · {} kbps",
            metrics.codec,
            metrics.width,
            metrics.height,
            metrics.fps,
            metrics.bitrate_bps / 1000,
        ),
        format!(
            "backend {} · capture {} fps · encode {} fps",
            metrics.backend, s.sender_fps, s.encoded_fps,
        ),
        format!(
            "encode avg {:.2} ms · {} B/s",
            s.encode_time_avg_us as f64 / 1000.0,
            s.bitrate_bps / 8,
        ),
        format!(
            "queue {} · dropped {}",
            s.send_queue_depth, s.dropped_frames,
        ),
        format!(
            "decode {} fps · latency ~{:.0} ms",
            s.receiver_fps,
            s.frame_age_us as f64 / 1000.0,
        ),
    ]
}

/// Developer diagnostics overlay pinned to the top-right of the viewer
/// surface (PDF Phase 12, behind the `screen_share_dev_overlay` flag).
/// Non-interactive: mouse events fall through to the surface below.
/// Takes ownership of the metrics so the returned element is `'static`
/// (no borrowed locals escape the view function).
pub(crate) fn view_screen_share_metrics_overlay(
    metrics: ScreenShareSessionMetrics,
) -> iced::Element<'static, AppMessage> {
    use iced::widget::{column, container, text};
    use iced::Length;

    let lines = screen_share_metrics_lines(&metrics);
    let col = column(
        lines
            .into_iter()
            .map(|line| text(line).size(10).color(iced::Color::WHITE).into())
            .collect::<Vec<iced::Element<'static, AppMessage>>>(),
    )
    .spacing(2);
    let panel = container(col)
        .padding(6)
        .style(|_| iced::widget::container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba8(
                0, 0, 0, 0.55,
            ))),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top)
        .padding(6)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: f32, h: f32) -> iced::Size {
        iced::Size::new(w, h)
    }

    #[test]
    fn fit_scale_preserves_aspect() {
        // 16:9 source in a 4:3 viewport: width-limited.
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Fit,
            None,
        );
        let s = g.fit_scale();
        assert!((s - 800.0 / 640.0).abs() < 1e-4);
        let d = g.display_size();
        assert!((d.width / d.height - 640.0 / 360.0).abs() < 1e-4);
        assert!(d.width <= 800.0 + 0.5 && d.height <= 600.0 + 0.5);
        assert!(g.fits());
    }

    #[test]
    fn wide_source_letterboxes_fit() {
        // 2:1 source in a 4:3 viewport: width-limited, vertical letterbox.
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(1000.0, 500.0),
            ScreenShareViewMode::Fit,
            None,
        );
        let d = g.display_size();
        assert!((d.width - 800.0).abs() < 1e-3);
        assert!((d.height - 400.0).abs() < 1e-3);
        assert!(g.fits());
    }

    #[test]
    fn actual_100_is_one_to_one_when_source_smaller_than_viewport() {
        let g = SurfaceGeometry::new(
            size(1200.0, 800.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Actual,
            None,
        );
        assert!((g.scale() - 1.0).abs() < 1e-6);
        let d = g.display_size();
        assert!((d.width - 640.0).abs() < 1e-3 && (d.height - 360.0).abs() < 1e-3);
        assert!(g.fits());
        // Centered rect
        let r = g.display_rect();
        assert!((r.x - (1200.0 - 640.0) / 2.0).abs() < 1e-3);
        assert!((r.y - (800.0 - 360.0) / 2.0).abs() < 1e-3);
    }

    #[test]
    fn actual_100_pans_when_source_larger_than_viewport() {
        let g = SurfaceGeometry::new(
            size(400.0, 300.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Actual,
            None,
        );
        assert!(!g.fits());
        // Visible region is viewport-sized in source px, centered.
        let (x, y, w, h) = g.visible_region();
        assert!((w - 400.0).abs() < 1e-3 && (h - 300.0).abs() < 1e-3);
        assert!((x - (640.0 - 400.0) / 2.0).abs() < 1e-3);
        assert!((y - (360.0 - 300.0) / 2.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_in_crops_and_pan_center_moves_region() {
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Zoom(2.0),
            None,
        );
        assert!(!g.fits());
        // At 2x, visible region is half the source.
        let (x, y, w, h) = g.visible_region();
        assert!((w - 400.0).abs() < 1e-3 && (h - 300.0).abs() < 1e-3);
        assert!((x - (640.0 - 400.0) / 2.0).abs() < 1e-3);
        // Pan to top-left corner.
        let g2 = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Zoom(2.0),
            Some((0.0, 0.0)),
        );
        let (x2, y2, _, _) = g2.visible_region();
        assert!(x2.abs() < 1e-3 && y2.abs() < 1e-3);
    }

    #[test]
    fn viewport_point_maps_to_source_and_normalized() {
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Fit,
            None,
        );
        // Center of viewport = center of source.
        let (sx, sy) = g.viewport_to_source(iced::Point::new(400.0, 300.0));
        assert!((sx - 320.0).abs() < 1e-3 && (sy - 180.0).abs() < 1e-3);
        // Top-left of the fitted display rect maps to source (0,0).
        let r = g.display_rect();
        let (sx2, sy2) = g.viewport_to_source(r.position());
        assert!(sx2.abs() < 1e-3 && sy2.abs() < 1e-3);
        // Normalized round trip.
        let (nx, ny) = g.viewport_to_normalized(iced::Point::new(400.0, 300.0));
        assert!((nx - 0.5).abs() < 1e-3 && (ny - 0.5).abs() < 1e-3);
    }

    #[test]
    fn viewport_point_maps_correctly_when_panned() {
        // Zoomed 2x with pan at top-left: viewport center shows source (200,150).
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Zoom(2.0),
            Some((200.0, 150.0)),
        );
        let (sx, sy) = g.viewport_to_source(iced::Point::new(400.0, 300.0));
        assert!((sx - 200.0).abs() < 1e-3 && (sy - 150.0).abs() < 1e-3);
        // Viewport top-left shows source top-left of the visible region.
        let (x, y, _, _) = g.visible_region();
        let (sx2, sy2) = g.viewport_to_source(iced::Point::new(0.0, 0.0));
        assert!((sx2 - x).abs() < 1e-3 && (sy2 - y).abs() < 1e-3);
    }

    #[test]
    fn zoom_anchored_at_cursor_keeps_source_point_under_cursor() {
        let viewport = size(800.0, 600.0);
        let source = size(640.0, 360.0);
        let anchor = iced::Point::new(600.0, 450.0); // right of center
        let g = SurfaceGeometry::new(
            viewport,
            source,
            ScreenShareViewMode::Zoom(2.0),
            Some((320.0, 180.0)),
        );
        let (before_x, before_y) = g.viewport_to_source(anchor);
        let new_pan = g.pan_for_zoom(anchor, 4.0);
        let g2 = SurfaceGeometry::new(
            viewport,
            source,
            ScreenShareViewMode::Zoom(4.0),
            Some(new_pan),
        );
        let (after_x, after_y) = g2.viewport_to_source(anchor);
        assert!((before_x - after_x).abs() < 1e-2, "{before_x} != {after_x}");
        assert!((before_y - after_y).abs() < 1e-2, "{before_y} != {after_y}");
    }

    #[test]
    fn pan_clamped_to_source_bounds() {
        // Pan far outside the source clamps so the region stays in bounds.
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(640.0, 360.0),
            ScreenShareViewMode::Zoom(4.0),
            Some((9999.0, 9999.0)),
        );
        let (x, y, w, h) = g.visible_region();
        assert!(x + w <= 640.0 + 1e-3 && y + h <= 360.0 + 1e-3);
        assert!(x >= 0.0 && y >= 0.0);
    }

    #[test]
    fn zero_sized_source_does_not_panic() {
        let g = SurfaceGeometry::new(
            size(800.0, 600.0),
            size(0.0, 0.0),
            ScreenShareViewMode::Fit,
            None,
        );
        assert_eq!(g.fit_scale(), 1.0);
        assert!(g.fits());
        let (nx, ny) = g.viewport_to_normalized(iced::Point::new(10.0, 10.0));
        assert_eq!(nx, 0.0);
        assert_eq!(ny, 0.0);
    }
}
