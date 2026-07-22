//! QR code generation and decoding for peer invitations.
//!
//! This module provides two main functions:
//!
//! * [`invitation_qr_png`] — encode an invitation URI as a QR code PNG image.
//! * [`decode_invitation_qr`] — scan a QR code image back into an invitation URI.
//!
//! The decoded text is validated through [`PeerInvitation::from_uri`] so that
//! malformed, oversized, or expired invitations are caught early.  Callers
//! should still call [`PeerInvitation::validate`] with their own public key
//! to detect self-invitations.
//!
//! # Image format support
//!
//! Decoding supports any format the [`image`] crate can open: PNG, JPEG,
//! WebP, GIF, BMP, TIFF, and others.  Generation always produces PNG.
//!
//! # Security
//!
//! QR contents are never trusted directly.  The decoded text is always
//! validated through [`PeerInvitation::from_uri`] which checks base64
//! well-formedness, payload size bounds, and structural validity.

use crate::peer_invitation;

/// Generate a QR code PNG image from an invitation URI.
///
/// `invitation` should be a full `boru-chat://pair/...` URI (as returned by
/// [`PeerInvitation::to_uri`]).
///
/// `size` is the desired minimum pixel dimension of the output image.  The
/// actual output may be slightly larger because QR codes have a fixed module
/// count.  A size of at least 512 is recommended for reliable scanning.
///
/// Returns the raw PNG bytes.
///
/// # Errors
///
/// Returns [`QrError::QrEncode`] if the input string is too long to fit in a
/// QR code or the QR encoder fails for another reason.
pub fn invitation_qr_png(invitation: &str, size: u32) -> Result<Vec<u8>, QrError> {
    let code = qrcode::QrCode::new(invitation.as_bytes())
        .map_err(|e| QrError::QrEncode(format!("failed to create QR code: {e}")))?;

    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(size, size)
        .dark_color(image::Luma([0u8]))
        .light_color(image::Luma([255u8]))
        .build();

    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| QrError::QrEncode(format!("failed to encode PNG: {e}")))?;

    Ok(png_bytes)
}

/// Decode a QR code image back into an invitation URI.
///
/// Accepts PNG, JPEG, WebP, and any other format supported by the [`image`]
/// crate.
///
/// The decoded text is validated through [`PeerInvitation::from_uri`], which
/// checks base64 well-formedness, payload size, and structural validity.
/// Callers should additionally call [`PeerInvitation::validate`] with their
/// own public key to detect self-invitations.
///
/// # Errors
///
/// Returns an error if:
/// * The image format is unsupported or corrupt.
/// * No QR code is found in the image.
/// * Multiple QR codes are found.
/// * The decoded text is not a valid `boru-chat://pair/...` invitation URI.
/// * The invitation has expired.
/// * The invitation uses an unsupported protocol version.
pub fn decode_invitation_qr(image_bytes: &[u8]) -> Result<String, QrError> {
    // 1. Decode the image from raw bytes.
    let img = image::load_from_memory(image_bytes)
        .map_err(|e| QrError::ImageDecode(format!("unsupported or corrupt image: {e}")))?;

    // 2. Convert to grayscale for QR detection.
    let gray = img.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(gray);
    let grids = prepared.detect_grids();

    // 3. Validate QR count.
    let content = match grids.len() {
        0 => return Err(QrError::NoQRCodeFound),
        1 => {
            let (_meta, content) = grids[0]
                .decode()
                .map_err(|e| QrError::QrDecode(format!("failed to decode QR code: {e}")))?;
            content
        }
        n => return Err(QrError::MultipleQRCodesFound(n)),
    };

    // 4. Validate through PeerInvitation (never trust QR contents directly).
    let inv =
        peer_invitation::PeerInvitation::from_uri(&content).ok_or(QrError::MalformedInvitation)?;

    // 5. Run structural validation (version, display name, addresses, expiry).
    inv.validate(None).map_err(|e| map_validation_error(&e))?;

    Ok(content)
}

/// Map a [`peer_invitation::ValidationError`] to a user-friendly [`QrError`].
fn map_validation_error(err: &peer_invitation::ValidationError) -> QrError {
    use peer_invitation::ValidationError as Ve;
    match err {
        Ve::UnsupportedVersion(v) => QrError::UnsupportedVersion(*v),
        Ve::Expired => QrError::ExpiredInvitation,
        // Structural issues (display name, addresses) are treated as
        // malformed invitations — the QR content decoded but didn't
        // produce a valid invitation.
        _ => QrError::MalformedInvitation,
    }
}

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during QR-code generation or decoding.
#[derive(Debug)]
pub enum QrError {
    /// Image could not be decoded (unsupported or corrupt format).
    ImageDecode(String),
    /// QR-code encoding or PNG rendering failed.
    QrEncode(String),
    /// QR-code scanning or content decoding failed.
    QrDecode(String),
    /// No QR code was found in the image.
    NoQRCodeFound,
    /// Multiple QR codes were found (expected exactly one).
    MultipleQRCodesFound(usize),
    /// The decoded text is not a valid peer invitation URI.
    MalformedInvitation,
    /// The invitation has expired.
    ExpiredInvitation,
    /// The invitation uses an unsupported protocol version.
    UnsupportedVersion(u8),
}

impl std::fmt::Display for QrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ImageDecode(msg) => write!(f, "unsupported or corrupt image: {msg}"),
            Self::QrEncode(msg) => write!(f, "QR code generation failed: {msg}"),
            Self::QrDecode(msg) => write!(f, "QR code decoding failed: {msg}"),
            Self::NoQRCodeFound => write!(f, "no QR code found in the image"),
            Self::MultipleQRCodesFound(n) => {
                write!(f, "found {n} QR codes (expected exactly one)")
            }
            Self::MalformedInvitation => {
                write!(f, "decoded text is not a valid peer invitation URI")
            }
            Self::ExpiredInvitation => write!(f, "invitation has expired"),
            Self::UnsupportedVersion(v) => {
                write!(f, "unsupported invitation version: {v}")
            }
        }
    }
}

impl std::error::Error for QrError {}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    /// Generate a valid invitation URI for testing.
    fn test_invitation_uri() -> String {
        let sk = iroh::SecretKey::generate();
        let inv = peer_invitation::PeerInvitation {
            version: 1,
            peer_id: sk.public(),
            display_name: "QR Test".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        };
        inv.to_uri().expect("to_uri should succeed")
    }

    // ── Generation tests ─────────────────────────────────────────────────

    #[test]
    fn qr_png_is_produced() {
        let uri = test_invitation_uri();
        let png = invitation_qr_png(&uri, 512).expect("qr generation should succeed");
        assert!(!png.is_empty(), "PNG data must not be empty");
    }

    #[test]
    fn qr_png_has_correct_dimensions() {
        let uri = test_invitation_uri();
        let png = invitation_qr_png(&uri, 512).expect("qr generation should succeed");

        // Decode the PNG and check dimensions.
        let img = image::load_from_memory(&png).expect("re-encode PNG should succeed");
        let (w, h) = img.dimensions();
        assert!(w >= 512, "width should be at least 512, got {w}");
        assert!(h >= 512, "height should be at least 512, got {h}");
    }

    #[test]
    fn qr_png_can_be_decoded_back() {
        let uri = test_invitation_uri();
        let png = invitation_qr_png(&uri, 512).expect("qr generation should succeed");

        let decoded = decode_invitation_qr(&png).expect("qr decode should succeed");
        assert_eq!(decoded, uri, "decoded URI must match original");
    }

    #[test]
    fn qr_png_with_small_size() {
        let uri = test_invitation_uri();
        // Even a small minimum size should produce a valid QR.
        let png = invitation_qr_png(&uri, 128).expect("qr generation should succeed");
        let decoded = decode_invitation_qr(&png).expect("qr decode should succeed");
        assert_eq!(decoded, uri);
    }

    #[test]
    fn qr_invitation_with_all_fields_round_trips() {
        let sk = iroh::SecretKey::generate();
        let inv = peer_invitation::PeerInvitation {
            version: 1,
            peer_id: sk.public(),
            display_name: "Full QR Test".to_string(),
            avatar_hash: Some("sha3:abc123".to_string()),
            relay_urls: vec!["relay.example.com:443".to_string()],
            direct_addresses: vec!["192.168.1.1:9000".to_string()],
            friend_request_token: Some("tok_qr_test".to_string()),
            expires_at: Some(i64::MAX),
        };
        let uri = inv.to_uri().expect("to_uri should succeed");
        let png = invitation_qr_png(&uri, 512).expect("qr generation should succeed");
        let decoded = decode_invitation_qr(&png).expect("qr decode should succeed");
        assert_eq!(decoded, uri);
    }

    // ── Decoding error tests ────────────────────────────────────────────

    #[test]
    fn decode_rejects_empty_image() {
        let err = decode_invitation_qr(b"").unwrap_err();
        assert!(
            matches!(err, QrError::ImageDecode(_)),
            "expected ImageDecode, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_junk_bytes() {
        let err = decode_invitation_qr(b"this is not an image").unwrap_err();
        assert!(
            matches!(err, QrError::ImageDecode(_)),
            "expected ImageDecode, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_image_with_no_qr() {
        // A small valid PNG with no QR code in it.
        let img = image::ImageBuffer::from_pixel(16, 16, image::Luma([128u8]));
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .expect("write PNG");
        let err = decode_invitation_qr(&png_bytes).unwrap_err();
        assert!(
            matches!(err, QrError::NoQRCodeFound),
            "expected NoQRCodeFound, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_expired_invitation() {
        let sk = iroh::SecretKey::generate();
        let inv = peer_invitation::PeerInvitation {
            version: 1,
            peer_id: sk.public(),
            display_name: "Expired".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: Some(1), // far in the past
        };
        let uri = inv.to_uri().expect("to_uri should succeed");
        let png = invitation_qr_png(&uri, 256).expect("qr generation should succeed");
        let err = decode_invitation_qr(&png).unwrap_err();
        assert!(
            matches!(err, QrError::ExpiredInvitation),
            "expected ExpiredInvitation, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_unsupported_version() {
        let sk = iroh::SecretKey::generate();
        let inv = peer_invitation::PeerInvitation {
            version: 99,
            peer_id: sk.public(),
            display_name: "BadVersion".to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: None,
        };
        let uri = inv.to_uri().expect("to_uri should succeed");
        let png = invitation_qr_png(&uri, 256).expect("qr generation should succeed");
        let err = decode_invitation_qr(&png).unwrap_err();
        assert!(
            matches!(err, QrError::UnsupportedVersion(99)),
            "expected UnsupportedVersion(99), got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_malformed_text_qr() {
        // Generate a QR code from plain text (not a valid invitation URI).
        let code = qrcode::QrCode::new(b"not-a-valid-invitation").expect("qr encode");
        let img = code
            .render::<image::Luma<u8>>()
            .min_dimensions(128, 128)
            .build();
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .expect("write PNG");
        let err = decode_invitation_qr(&png_bytes).unwrap_err();
        assert!(
            matches!(err, QrError::MalformedInvitation),
            "expected MalformedInvitation, got {err:?}"
        );
    }

    #[test]
    fn decode_round_trip_jpeg() {
        let uri = test_invitation_uri();
        let png = invitation_qr_png(&uri, 512).expect("qr generation should succeed");

        // Decode the PNG to pixels, then re-encode as JPEG.
        let img = image::load_from_memory(&png).expect("load PNG");
        let mut jpeg_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut jpeg_bytes),
            image::ImageFormat::Jpeg,
        )
        .expect("write JPEG");

        let decoded =
            decode_invitation_qr(&jpeg_bytes).expect("qr decode from JPEG should succeed");
        assert_eq!(decoded, uri, "decoded URI must match original from JPEG");
    }

    #[test]
    fn decode_round_trip_webp() {
        let uri = test_invitation_uri();
        let png = invitation_qr_png(&uri, 512).expect("qr generation should succeed");

        let img = image::load_from_memory(&png).expect("load PNG");
        let mut webp_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut webp_bytes),
            image::ImageFormat::WebP,
        )
        .expect("write WebP");

        let decoded =
            decode_invitation_qr(&webp_bytes).expect("qr decode from WebP should succeed");
        assert_eq!(decoded, uri, "decoded URI must match original from WebP");
    }

    #[test]
    fn error_display_is_user_friendly() {
        // Spot-check that Display impls look friendly.
        assert_eq!(
            QrError::NoQRCodeFound.to_string(),
            "no QR code found in the image"
        );
        assert_eq!(
            QrError::ExpiredInvitation.to_string(),
            "invitation has expired"
        );
        assert_eq!(
            QrError::MalformedInvitation.to_string(),
            "decoded text is not a valid peer invitation URI"
        );
        let multi = QrError::MultipleQRCodesFound(3);
        assert_eq!(multi.to_string(), "found 3 QR codes (expected exactly one)");
    }
}
