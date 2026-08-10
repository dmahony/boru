//! SPAKE2 short-code pairing — a password-authenticated key-exchange
//! complement to QR/URI peer invitations.
//!
//! # Motivation
//!
//! Boru's existing out-of-band pairing flow
//! ([`crate::peer_invitation`] + [`crate::pairing_service`]) exchanges a
//! [`PeerInvitation`](crate::peer_invitation::PeerInvitation) over a QR code or `boru-chat://pair/...` URI.  SPAKE2
//! adds a **short numeric code** path that is useful when both peers are in
//! the same room: they agree on a 6-digit code and both sides authenticate
//! each other with the code as the shared password.
//!
//! # Protocol
//!
//! 1. Both peers agree on a short code (e.g. `123456`) out-of-band.
//! 2. Each side calls [`Spake2Pairing::begin`](crate::spake2_pairing::Spake2Pairing::begin) with the same code and the
//!    same identity context.  Each receives an outbound message.
//! 3. The outbound messages are exchanged (over whisper, clipboard, or any
//!    transport the caller chooses).
//! 4. Each side calls [`Spake2Pairing::finish`](crate::spake2_pairing::Spake2Pairing::finish) with the *other* side's
//!    outbound message.  Both derive the **same** shared key iff they used
//!    the same code (SPAKE2 is a PAKE — an attacker who does not know the
//!    code cannot learn the key).
//! 5. Each side wraps its [`PeerInvitation`](crate::peer_invitation::PeerInvitation) with a keyed MAC using the
//!    shared key ([`Spake2SharedKey::authenticate`](crate::spake2_pairing::Spake2SharedKey::authenticate)) and sends the result.
//! 6. Each side verifies the peer's authenticated invitation
//!    ([`Spake2SharedKey::verify`](crate::spake2_pairing::Spake2SharedKey::verify)) and feeds it into the existing
//!    [`accept_peer_invitation`](crate::pairing_service::accept_peer_invitation) flow, which creates the friend record and
//!    persists a [`PendingPairing`](crate::pairing_service::PendingPairing) for restart recovery.
//!
//! # Security notes
//!
//! - The short code is the only secret.  Use codes with sufficient entropy
//!   for the threat model (6 digits ≈ 20 bits, adequate for a same-room
//!   pairing that completes immediately; SPAKE2 prevents offline guessing).
//! - The identity context string must be identical on both sides and should
//!   be a stable protocol constant (it is *not* secret).
//! - The MAC key is derived from the SPAKE2 shared secret via HKDF-SHA256,
//!   so the invitation exchange is authenticated and tamper-evident.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::pairing_service::{accept_peer_invitation, PairingContext, PairingResult};
use crate::peer_invitation::PeerInvitation;

/// Length of a short code in digits.
pub const SHORT_CODE_LEN: usize = 6;

/// Length of the derived shared key (and MAC output) in bytes.
const KEY_LEN: usize = 32;

/// The MAC algorithm used to authenticate invitations with the shared key.
type HmacSha256 = Hmac<Sha256>;

/// Protocol identity context shared by both peers.
///
/// This is a stable, non-secret constant that binds both SPAKE2 sessions to
/// the same protocol.  It must match on both sides or the key exchange fails.
const SPAKE2_CONTEXT: &[u8] = b"boru-chat short-code pairing v1";

/// Errors from short-code pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spake2Error {
    /// The code is not exactly `SHORT_CODE_LEN` decimal digits.
    InvalidCode,
    /// The peer's SPAKE2 message could not be processed (bad side/length).
    InvalidPeerMessage,
    /// The authenticated invitation failed MAC verification.
    AuthenticationFailed,
    /// The authenticated invitation payload could not be decoded.
    InvalidInvitation,
}

impl std::fmt::Display for Spake2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Spake2Error::InvalidCode => {
                write!(f, "short code must be {SHORT_CODE_LEN} decimal digits")
            }
            Spake2Error::InvalidPeerMessage => {
                write!(f, "peer SPAKE2 message is invalid")
            }
            Spake2Error::AuthenticationFailed => {
                write!(
                    f,
                    "peer invitation authentication failed (code mismatch or tampering)"
                )
            }
            Spake2Error::InvalidInvitation => {
                write!(f, "authenticated invitation payload is invalid")
            }
        }
    }
}

impl std::error::Error for Spake2Error {}

/// A 6-digit short code used as the SPAKE2 password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortCode {
    digits: String,
}

impl ShortCode {
    /// Create a short code from a decimal string.
    ///
    /// Returns [`Spake2Error::InvalidCode`] unless the input is exactly
    /// `SHORT_CODE_LEN` ASCII decimal digits.
    pub fn parse(input: &str) -> Result<Self, Spake2Error> {
        if input.len() != SHORT_CODE_LEN || !input.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Spake2Error::InvalidCode);
        }
        Ok(Self {
            digits: input.to_string(),
        })
    }

    /// Generate a fresh random short code using the OS CSPRNG.
    ///
    /// Each digit is drawn from a uniform distribution over 0–9.
    pub fn generate() -> Result<Self, Spake2Error> {
        let mut bytes = [0u8; SHORT_CODE_LEN];
        getrandom::fill(&mut bytes).map_err(|_| Spake2Error::InvalidCode)?;
        let digits: String = bytes.iter().map(|b| char::from(b'0' + (b % 10))).collect();
        Ok(Self { digits })
    }

    /// The code as bytes (used as the SPAKE2 password).
    pub fn as_bytes(&self) -> &[u8] {
        self.digits.as_bytes()
    }

    /// The code as a string (for display / entry).
    pub fn as_str(&self) -> &str {
        &self.digits
    }
}

/// A SPAKE2 pairing session for one side of the exchange.
pub struct Spake2Pairing {
    inner: spake2::Spake2<spake2::Ed25519Group>,
}

impl std::fmt::Debug for Spake2Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately redacted: the session holds SPAKE2 state that must not
        // leak into logs (it derives from the shared short code).
        f.debug_struct("Spake2Pairing")
            .field("inner", &"[redacted]")
            .finish()
    }
}

impl Spake2Pairing {
    /// Begin a pairing session with the given shared code.
    ///
    /// Returns `(session, outbound_message)`.  Send `outbound_message` to the
    /// peer, then call [`Self::finish`] with the peer's outbound message.
    pub fn begin(code: &ShortCode) -> (Self, Vec<u8>) {
        let (inner, outbound) = spake2::Spake2::<spake2::Ed25519Group>::start_symmetric(
            &spake2::Password::new(code.as_bytes()),
            &spake2::Identity::new(SPAKE2_CONTEXT),
        );
        (Self { inner }, outbound)
    }

    /// Finish the exchange with the peer's outbound message.
    ///
    /// Both sides derive the same shared key iff they used the same code.
    pub fn finish(self, peer_outbound: &[u8]) -> Result<Spake2SharedKey, Spake2Error> {
        let raw = self
            .inner
            .finish(peer_outbound)
            .map_err(|_| Spake2Error::InvalidPeerMessage)?;
        // Expand the raw SPAKE2 secret into a fixed-length key with HKDF.
        let hk = Hkdf::<Sha256>::new(None, &raw);
        let mut key = [0u8; KEY_LEN];
        hk.expand(SPAKE2_CONTEXT, &mut key)
            .map_err(|_| Spake2Error::AuthenticationFailed)?;
        Ok(Spake2SharedKey(key))
    }
}

/// The shared key derived from a successful SPAKE2 exchange.
///
/// Both sides obtain the identical key only if they used the same short code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spake2SharedKey([u8; KEY_LEN]);

impl Spake2SharedKey {
    /// Authenticate a local [`PeerInvitation`] with the shared key.
    ///
    /// The result is sent to the peer, who verifies it with
    /// [`Self::verify`].  An attacker who does not know the code cannot
    /// forge a valid authenticated invitation.
    pub fn authenticate(
        &self,
        invitation: &PeerInvitation,
    ) -> Result<AuthenticatedInvitation, Spake2Error> {
        let payload =
            postcard::to_allocvec(invitation).map_err(|_| Spake2Error::InvalidInvitation)?;
        let mac = self.mac(&payload);
        Ok(AuthenticatedInvitation { payload, mac })
    }

    /// Verify and decode a peer's authenticated invitation.
    ///
    /// Returns the verified [`PeerInvitation`] on success.  Fails when the
    /// MAC does not match (wrong code, tampering) or the payload is not a
    /// valid invitation.
    pub fn verify(&self, authed: &AuthenticatedInvitation) -> Result<PeerInvitation, Spake2Error> {
        let expected = self.mac(&authed.payload);
        if expected != authed.mac {
            return Err(Spake2Error::AuthenticationFailed);
        }
        postcard::from_bytes(&authed.payload).map_err(|_| Spake2Error::InvalidInvitation)
    }

    /// Compute the HMAC-SHA256 of `data` under the shared key.
    fn mac(&self, data: &[u8]) -> [u8; KEY_LEN] {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("hmac accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().into()
    }
}

/// A [`PeerInvitation`] authenticated with a SPAKE2 shared key.
///
/// Wire format: `payload` is the postcard-encoded [`PeerInvitation`], `mac`
/// is HMAC-SHA256 over `payload` under the shared key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedInvitation {
    /// Postcard-encoded [`PeerInvitation`].
    pub payload: Vec<u8>,
    /// HMAC-SHA256 of `payload` under the shared key.
    pub mac: [u8; KEY_LEN],
}

/// Complete a short-code pairing after verifying the peer's authenticated
/// invitation.
///
/// This mirrors [`accept_peer_invitation`]: it validates the invitation,
/// updates the friends store with the peer's addresses, creates a pending
/// friend request, signs it, and persists a [`PendingPairing`](crate::pairing_service::PendingPairing) for restart
/// recovery.  The existing URI/QR flow is untouched; this is an additive
/// entry point for the SPAKE2 path.
///
/// Returns the same [`PairingResult`] as the QR/URI flow so callers can
/// reuse the same UI handling.
pub fn accept_spake2_pairing(
    authed: &AuthenticatedInvitation,
    key: &Spake2SharedKey,
    context: &PairingContext,
    friends_store: &mut crate::friends::FriendsStore,
    friend_request_store: &mut crate::friend_request::FriendRequestStore,
) -> n0_error::Result<PairingResult> {
    let invitation = key
        .verify(authed)
        .map_err(|e| n0_error::anyerr!("SPAKE2 invitation verification failed: {e}"))?;
    accept_peer_invitation(&invitation, context, friends_store, friend_request_store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    /// Run a full two-sided SPAKE2 exchange with the given codes.
    fn exchange(alice_code: &str, bob_code: &str) -> (Spake2SharedKey, Spake2SharedKey) {
        let alice_code = ShortCode::parse(alice_code).expect("alice code");
        let bob_code = ShortCode::parse(bob_code).expect("bob code");

        let (alice_session, alice_msg) = Spake2Pairing::begin(&alice_code);
        let (bob_session, bob_msg) = Spake2Pairing::begin(&bob_code);

        let alice_key = alice_session.finish(&bob_msg).expect("alice finish");
        let bob_key = bob_session.finish(&alice_msg).expect("bob finish");
        (alice_key, bob_key)
    }

    fn make_invitation(name: &str, sk: &SecretKey) -> PeerInvitation {
        PeerInvitation {
            version: 1,
            peer_id: sk.public(),
            display_name: name.to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: Some(i64::MAX),
        }
    }

    #[test]
    fn test_short_code_validation() {
        assert!(ShortCode::parse("123456").is_ok());
        assert!(ShortCode::parse("000000").is_ok());
        assert_eq!(ShortCode::parse("12345"), Err(Spake2Error::InvalidCode));
        assert_eq!(ShortCode::parse("1234567"), Err(Spake2Error::InvalidCode));
        assert_eq!(ShortCode::parse("abcdef"), Err(Spake2Error::InvalidCode));
        assert_eq!(ShortCode::parse("12 456"), Err(Spake2Error::InvalidCode));
        assert_eq!(ShortCode::parse(""), Err(Spake2Error::InvalidCode));
    }

    #[test]
    fn test_short_code_generate_is_six_digits() {
        for _ in 0..5 {
            let code = ShortCode::generate().expect("generate");
            assert_eq!(code.as_str().len(), SHORT_CODE_LEN);
            assert!(code.as_str().bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn test_matching_codes_derive_same_key() {
        let (alice_key, bob_key) = exchange("123456", "123456");
        assert_eq!(alice_key, bob_key, "same code ⇒ same shared key");
    }

    #[test]
    fn test_different_codes_derive_different_keys() {
        let (alice_key, bob_key) = exchange("123456", "654321");
        assert_ne!(
            alice_key, bob_key,
            "different codes must not derive the same key"
        );
    }

    #[test]
    fn test_invitation_roundtrip_and_tamper_detection() {
        let (alice_key, bob_key) = exchange("123456", "123456");
        let alice_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &alice_sk);

        let authed = alice_key.authenticate(&invitation).expect("authenticate");
        let verified = bob_key.verify(&authed).expect("verify with same key");
        assert_eq!(verified, invitation, "verified invitation matches");

        // Tamper with the payload → verification must fail.
        let mut tampered = authed.clone();
        tampered.payload[0] ^= 0xff;
        let err = bob_key
            .verify(&tampered)
            .expect_err("tampered payload rejected");
        assert_eq!(err, Spake2Error::AuthenticationFailed);

        // Wrong key (code mismatch) → verification must fail.
        let (_, wrong_key) = exchange("111111", "123456");
        let err = wrong_key.verify(&authed).expect_err("wrong key rejected");
        assert_eq!(err, Spake2Error::AuthenticationFailed);
    }

    #[test]
    fn test_spake2_pairing_accept_flow() {
        let (alice_key, bob_key) = exchange("123456", "123456");
        let alice_sk = SecretKey::generate();
        let bob_sk = SecretKey::generate();

        let alice_inv = make_invitation("Alice", &alice_sk);
        let bob_inv = make_invitation("Bob", &bob_sk);

        // Each side authenticates its own invitation for the peer.
        let alice_authed = alice_key.authenticate(&alice_inv).expect("alice authed");
        let bob_authed = bob_key.authenticate(&bob_inv).expect("bob authed");

        // Each side verifies the peer's invitation.
        let verified_bob = alice_key.verify(&bob_authed).expect("alice verifies bob");
        let verified_alice = bob_key.verify(&alice_authed).expect("bob verifies alice");
        assert_eq!(verified_bob, bob_inv);
        assert_eq!(verified_alice, alice_inv);

        // The SPAKE2 accept flow delegates to accept_peer_invitation with
        // the verified invitation — the full store/persistence behavior is
        // covered by pairing_service tests; here we confirm the delegation
        // plumbing by exercising it with a temp store.
        let dir = std::env::temp_dir().join(format!(
            "boru-spake2-accept-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");

        let mut friends = crate::friends::FriendsStore::empty_at(&dir);
        let mut requests = crate::friend_request::FriendRequestStore::empty_at(&dir);
        let context = PairingContext::new(bob_sk.clone(), "Bob", vec![], vec![]);

        let (outcome, signed_msg) = accept_spake2_pairing(
            &alice_authed,
            &bob_key,
            &context,
            &mut friends,
            &mut requests,
        )
        .expect("accept spake2 pairing");

        assert!(matches!(
            outcome,
            crate::pairing_service::PairingOutcome::RequestSent { .. }
        ));
        assert!(signed_msg.is_some(), "signed friend request ready to send");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
