//! Aspect-ratio-preserving geometry for live-call video.

/// The source rectangle after fitting inside a viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContainRect {
    /// Horizontal letterbox offset in viewport units.
    pub x: f32,
    /// Vertical letterbox offset in viewport units.
    pub y: f32,
    /// Fitted source width in viewport units.
    pub width: f32,
    /// Fitted source height in viewport units.
    pub height: f32,
}

/// Compute a centered contain-fit rectangle.
///
/// The whole source is visible and centered in the viewport. Empty space is
/// intentional letterboxing; no camera pixels are stretched or cropped.
pub fn contain_fit_rect(
    source_width: f32,
    source_height: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<ContainRect> {
    if !source_width.is_finite()
        || !source_height.is_finite()
        || !viewport_width.is_finite()
        || !viewport_height.is_finite()
        || source_width <= 0.0
        || source_height <= 0.0
        || viewport_width <= 0.0
        || viewport_height <= 0.0
    {
        return None;
    }

    let scale = (viewport_width / source_width).min(viewport_height / source_height);
    let width = source_width * scale;
    let height = source_height * scale;
    Some(ContainRect {
        x: (viewport_width - width) / 2.0,
        y: (viewport_height - height) / 2.0,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::{contain_fit_rect, ContainRect};

    fn assert_rect(actual: Option<ContainRect>, expected: ContainRect) {
        let actual = actual.expect("valid contain-fit dimensions");
        let epsilon = 0.001;
        assert!((actual.x - expected.x).abs() < epsilon, "x: {actual:?}");
        assert!((actual.y - expected.y).abs() < epsilon, "y: {actual:?}");
        assert!(
            (actual.width - expected.width).abs() < epsilon,
            "width: {actual:?}"
        );
        assert!(
            (actual.height - expected.height).abs() < epsilon,
            "height: {actual:?}"
        );
    }

    #[test]
    fn widescreen_source_is_letterboxed_in_portrait_viewport() {
        assert_rect(
            contain_fit_rect(16.0, 9.0, 4.0, 3.0),
            ContainRect {
                x: 0.0,
                y: 0.375,
                width: 4.0,
                height: 2.25,
            },
        );
    }

    #[test]
    fn four_three_source_is_letterboxed_in_widescreen_viewport() {
        assert_rect(
            contain_fit_rect(4.0, 3.0, 16.0, 9.0),
            ContainRect {
                x: 2.0,
                y: 0.0,
                width: 12.0,
                height: 9.0,
            },
        );
    }

    #[test]
    fn unusual_dimensions_preserve_ratio_and_center_the_frame() {
        assert_rect(
            contain_fit_rect(123.0, 77.0, 500.0, 300.0),
            ContainRect {
                x: 10.3896,
                y: 0.0,
                width: 479.2208,
                height: 300.0,
            },
        );
    }

    #[test]
    fn invalid_dimensions_are_rejected() {
        assert!(contain_fit_rect(0.0, 9.0, 4.0, 3.0).is_none());
        assert!(contain_fit_rect(16.0, f32::NAN, 4.0, 3.0).is_none());
    }
}
