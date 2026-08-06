#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use data_encoding::BASE64URL_NOPAD;
#[cfg(test)]
use image::{ImageBuffer, Luma};
#[cfg(test)]
use iroh::{EndpointAddr, PublicKey};
#[cfg(test)]
use n0_error::{bail_any, Result, StdResultExt};
#[cfg(test)]
use qrcode::QrCode;
#[cfg(test)]
use serde::{Deserialize, Serialize};

#[cfg(test)]
const INVITATION_URI_PREFIX: &str = "boru-chat://invite/";
#[cfg(test)]
const LEGACY_INVITATION_URI_PREFIX: &str = "boru-chat://pair/";
#[cfg(test)]
const INVITATION_VERSION: u8 = 1;
#[cfg(test)]
const DEFAULT_TTL_SECS: u64 = 24 * 60 * 60;
#[cfg(test)]
const MIN_QR_DIMENSION: u32 = 512;

/// Compact invitation payload shared via QR or paste.
///
/// This intentionally carries only the information required by the UI flow:
/// the sender's identity, a display label, a safe avatar ticket if available,
/// and any connection hints the user explicitly allowed to share.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationPayload {
    pub version: u8,
    pub public_key: PublicKey,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_mode: Option<String>,
    #[serde(default)]
    pub connection_hints: Vec<EndpointAddr>,
    pub pairing_token: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_secs: Option<u64>,
}

#[cfg(test)]
impl InvitationPayload {
    pub fn new(
        public_key: PublicKey,
        display_name: impl Into<String>,
        avatar_ticket: Option<String>,
        relay_mode: Option<String>,
        connection_hints: Vec<EndpointAddr>,
        expires_at_unix_secs: Option<u64>,
        pairing_token: u64,
    ) -> Self {
        Self {
            version: INVITATION_VERSION,
            public_key,
            display_name: display_name.into(),
            avatar_ticket,
            relay_mode,
            connection_hints,
            pairing_token,
            expires_at_unix_secs,
        }
    }

    pub fn new_ephemeral(
        public_key: PublicKey,
        display_name: impl Into<String>,
        avatar_ticket: Option<String>,
        relay_mode: Option<String>,
        connection_hints: Vec<EndpointAddr>,
        now_unix_secs: u64,
        pairing_token: u64,
    ) -> Self {
        Self::new(
            public_key,
            display_name,
            avatar_ticket,
            relay_mode,
            connection_hints,
            Some(now_unix_secs + DEFAULT_TTL_SECS),
            pairing_token,
        )
    }

    pub fn current_time_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn is_expired(&self, now_unix_secs: u64) -> bool {
        self.expires_at_unix_secs
            .is_some_and(|expires| now_unix_secs > expires)
    }

    pub fn short_public_key(&self) -> String {
        let pk = self.public_key.to_string();
        if pk.len() > 12 {
            format!("{}…", &pk[..12])
        } else {
            pk
        }
    }

    pub fn invitation_uri(&self) -> Result<String> {
        let payload = postcard::to_stdvec(self).std_context("encode invitation payload")?;
        Ok(format!(
            "{INVITATION_URI_PREFIX}{}",
            BASE64URL_NOPAD.encode(&payload)
        ))
    }

    pub fn invitation_preview(&self) -> String {
        let mut preview = format!("{} · {}", self.display_name.trim(), self.short_public_key());
        if let Some(relay_mode) = &self.relay_mode {
            preview.push_str(" · ");
            preview.push_str(relay_mode.trim());
        }
        preview
    }

    pub fn connection_summary(&self) -> String {
        if self.connection_hints.is_empty() {
            self.relay_mode
                .clone()
                .unwrap_or_else(|| "relay-only".to_string())
        } else {
            format!("{} hint(s)", self.connection_hints.len())
        }
    }

    pub fn expiry_summary(&self) -> Option<String> {
        self.expires_at_unix_secs.map(|expires| {
            let now = Self::current_time_unix_secs();
            if expires <= now {
                "expired".to_string()
            } else {
                let remaining = expires - now;
                let hours = remaining / 3600;
                let minutes = (remaining % 3600) / 60;
                if hours > 0 {
                    format!("expires in {hours}h {minutes}m")
                } else {
                    format!("expires in {minutes}m")
                }
            }
        })
    }

    pub fn render_qr_png(&self) -> Result<Vec<u8>> {
        let uri = self.invitation_uri()?;
        encode_qr_png(&uri)
    }

    pub fn from_invitation_input(input: &str) -> Result<Self> {
        let normalized = normalize_invitation_input(input);
        if normalized.is_empty() {
            bail_any!("invitation input is empty");
        }

        if let Some(payload) = normalized.strip_prefix(INVITATION_URI_PREFIX) {
            return Self::from_encoded_payload(payload);
        }
        if let Some(payload) = normalized.strip_prefix(LEGACY_INVITATION_URI_PREFIX) {
            return Self::from_encoded_payload(payload);
        }
        if let Some(payload) = normalized.strip_prefix("boru-chat://") {
            return Self::from_encoded_payload(payload);
        }
        if let Ok(public_key) = normalized.parse::<PublicKey>() {
            return Ok(Self::new(
                public_key,
                "Shared invitation".to_string(),
                None,
                None,
                Vec::new(),
                None,
                rand::random::<u64>(),
            ));
        }

        // Raw payload fallback for copy/paste flows that strip the URI prefix.
        if let Ok(payload) = BASE64URL_NOPAD.decode(normalized.as_bytes()) {
            if let Ok(invitation) = postcard::from_bytes::<Self>(&payload) {
                return invitation.validate();
            }
        }

        bail_any!("unsupported invitation format");
    }

    pub fn validate(self) -> Result<Self> {
        if self.version != INVITATION_VERSION {
            bail_any!(
                "unsupported invitation version {} (expected {})",
                self.version,
                INVITATION_VERSION
            );
        }
        if self.display_name.trim().is_empty() {
            bail_any!("invitation display name is empty");
        }
        if self.is_expired(Self::current_time_unix_secs()) {
            bail_any!("invitation has expired");
        }
        Ok(self)
    }

    fn from_encoded_payload(payload: &str) -> Result<Self> {
        let bytes = BASE64URL_NOPAD
            .decode(payload.as_bytes())
            .std_context("decode invitation payload")?;
        let invitation: Self =
            postcard::from_bytes(&bytes).std_context("decode invitation postcard payload")?;
        invitation.validate()
    }
}

#[cfg(test)]
pub fn normalize_invitation_input(input: &str) -> String {
    input.split_whitespace().collect::<String>()
}

#[cfg(test)]
pub fn encode_qr_png(uri: &str) -> Result<Vec<u8>> {
    let code = QrCode::new(uri.as_bytes()).std_context("build QR code")?;
    let image: ImageBuffer<Luma<u8>, Vec<u8>> = code
        .render::<Luma<u8>>()
        .min_dimensions(MIN_QR_DIMENSION, MIN_QR_DIMENSION)
        .quiet_zone(true)
        .build();

    let mut bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    use image::ImageEncoder as _;
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ColorType::L8.into(),
        )
        .std_context("encode QR PNG")?;
    Ok(bytes)
}

#[cfg(test)]
pub fn decode_qr_png(bytes: &[u8]) -> Result<String> {
    let image = image::load_from_memory(bytes).std_context("decode QR image")?;
    let gray = image.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(gray);
    let grids = prepared.detect_grids();
    if grids.is_empty() {
        bail_any!("no QR code found in the selected image");
    }
    if grids.len() > 1 {
        bail_any!("multiple QR codes found in the selected image");
    }
    let (_, content) = grids[0].decode().std_context("decode QR payload")?;
    Ok(content)
}

#[cfg(test)]
pub fn invitation_from_qr_png(bytes: &[u8]) -> Result<InvitationPayload> {
    let uri = decode_qr_png(bytes)?;
    InvitationPayload::from_invitation_input(&uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn make_payload(
        display_name: &str,
        expires_at: Option<u64>,
        pairing_token: u64,
    ) -> InvitationPayload {
        InvitationPayload::new(
            SecretKey::generate().public(),
            display_name,
            None,
            None,
            Vec::new(),
            expires_at,
            pairing_token,
        )
    }

    // ── Invitation creation / round-trip ────────────────────────────────

    #[test]
    fn invitation_uri_round_trips_and_qr_decodes() {
        let payload = InvitationPayload::new_ephemeral(
            SecretKey::generate().public(),
            "Alice".to_string(),
            Some("avatar-ticket".to_string()),
            Some("Default Relay".to_string()),
            Vec::new(),
            InvitationPayload::current_time_unix_secs(),
            42,
        );

        let uri = payload.invitation_uri().unwrap();
        assert!(uri.starts_with(INVITATION_URI_PREFIX));

        let decoded = InvitationPayload::from_invitation_input(&uri).unwrap();
        assert_eq!(decoded.display_name, "Alice");
        assert_eq!(decoded.pairing_token, 42);

        let png = payload.render_qr_png().unwrap();
        let from_qr = invitation_from_qr_png(&png).unwrap();
        assert_eq!(from_qr.public_key, payload.public_key);
        assert_eq!(from_qr.display_name, payload.display_name);
    }

    #[test]
    fn accepts_bare_public_key() {
        let public_key = SecretKey::generate().public();
        let parsed = InvitationPayload::from_invitation_input(&public_key.to_string()).unwrap();
        assert_eq!(parsed.public_key, public_key);
        assert_eq!(parsed.display_name, "Shared invitation");
    }

    #[test]
    fn reject_expired_invitation() {
        let expired = InvitationPayload::new(
            SecretKey::generate().public(),
            "Alice",
            None,
            None,
            Vec::new(),
            Some(InvitationPayload::current_time_unix_secs() - 1),
            7,
        );

        assert!(expired.validate().is_err());
    }

    // ── Show-My-QR-screen helpers ───────────────────────────────────────

    #[test]
    fn invitation_preview_contains_display_name() {
        let payload = make_payload("Bob", None, 1);
        let preview = payload.invitation_preview();
        assert!(preview.contains("Bob"), "preview must contain display name");
    }

    #[test]
    fn invitation_preview_contains_short_public_key() {
        let payload = make_payload("Charlie", None, 2);
        let preview = payload.invitation_preview();
        let short = payload.short_public_key();
        assert!(
            preview.contains(&short),
            "preview must contain short public key"
        );
    }

    #[test]
    fn invitation_preview_appends_relay_mode() {
        let payload = InvitationPayload::new(
            SecretKey::generate().public(),
            "RelayUser",
            None,
            Some("Default Relay".to_string()),
            Vec::new(),
            None,
            3,
        );
        let preview = payload.invitation_preview();
        assert!(preview.contains("Default Relay"));
    }

    #[test]
    fn expiry_summary_shows_remaining_time() {
        // Set expiry 2 hours in the future.
        let future = InvitationPayload::current_time_unix_secs() + 7200;
        let payload = make_payload("Timed", Some(future), 4);
        let summary = payload.expiry_summary();
        assert!(
            summary.is_some(),
            "expiry_summary must be Some for future expiry"
        );
        let text = summary.unwrap();
        assert!(
            text.contains("expires in"),
            "must say 'expires in', got: {text}"
        );
        assert!(text.contains("2h"), "must show 2h, got: {text}");
    }

    #[test]
    fn expiry_summary_returns_expired_when_past() {
        let past = InvitationPayload::current_time_unix_secs() - 3600;
        let payload = make_payload("Late", Some(past), 5);
        let summary = payload.expiry_summary();
        assert!(
            summary.is_some(),
            "expiry_summary must be Some for past expiry"
        );
        assert_eq!(summary.unwrap(), "expired");
    }

    #[test]
    fn expiry_summary_returns_none_when_no_expiry() {
        let payload = make_payload("Forever", None, 6);
        assert!(payload.expiry_summary().is_none());
    }

    #[test]
    fn connection_summary_without_hints_shows_relay_mode() {
        let payload = InvitationPayload::new(
            SecretKey::generate().public(),
            "RelayOnly",
            None,
            Some("VPS Relay".to_string()),
            Vec::new(),
            None,
            7,
        );
        assert_eq!(payload.connection_summary(), "VPS Relay");
    }

    #[test]
    fn connection_summary_with_hints_shows_count() {
        // Create an EndpointAddr from a generated public key.
        let pk = SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let payload = InvitationPayload::new(
            SecretKey::generate().public(),
            "DirectUser",
            None,
            Some("Relay".to_string()),
            vec![addr],
            None,
            8,
        );
        let summary = payload.connection_summary();
        assert!(summary.contains("1 hint(s)"), "got: {summary}");
    }

    #[test]
    fn connection_summary_relay_only_fallback() {
        let payload = make_payload("NoRelay", None, 9);
        assert_eq!(payload.connection_summary(), "relay-only");
    }

    #[test]
    fn short_public_key_truncates_long_keys() {
        let payload = make_payload("LongKey", None, 10);
        let short = payload.short_public_key();
        let char_count = short.chars().count();
        assert!(
            char_count <= 14,
            "short key should be at most 14 chars (12 hex + ellipsis), got {short} ({char_count} chars)"
        );
    }

    #[test]
    fn short_public_key_is_stable() {
        let payload = make_payload("Stable", None, 11);
        let first = payload.short_public_key();
        let second = payload.short_public_key();
        assert_eq!(first, second, "short key must be stable");
    }

    #[test]
    fn render_qr_png_is_valid_png() {
        let payload = make_payload("QRTest", None, 12);
        let png = payload.render_qr_png().expect("QR render must succeed");
        assert!(!png.is_empty(), "PNG data must not be empty");

        // Verify it's a valid PNG by decoding it back.
        let img = image::load_from_memory(&png).expect("PNG must be valid");
        assert!(img.width() >= 512, "QR width must be >= 512");
        assert!(img.height() >= 512, "QR height must be >= 512");
    }

    #[test]
    fn invitation_uri_has_correct_prefix() {
        let payload = make_payload("UriCheck", None, 13);
        let uri = payload
            .invitation_uri()
            .expect("invitation_uri must succeed");
        assert!(
            uri.starts_with(INVITATION_URI_PREFIX),
            "URI must start with {INVITATION_URI_PREFIX}, got: {uri}"
        );
    }

    #[test]
    fn from_invitation_input_rejects_empty_string() {
        let err = InvitationPayload::from_invitation_input("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn from_invitation_input_rejects_garbage() {
        let err = InvitationPayload::from_invitation_input("not-an-invitation-at-all").unwrap_err();
        assert!(err.to_string().contains("unsupported invitation format"));
    }
}
