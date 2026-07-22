use boru_core::outbox_delivery::{DeliveryError, DeliveryFailure, FailureClass};

#[test]
fn every_failure_has_stable_code_and_expected_classification() {
    let cases = [
        (
            DeliveryFailure::PeerOffline,
            "peer_offline",
            FailureClass::Transient,
        ),
        (
            DeliveryFailure::AddressUnavailable,
            "address_unavailable",
            FailureClass::Transient,
        ),
        (
            DeliveryFailure::ConnectionFailed,
            "connection_failed",
            FailureClass::Transient,
        ),
        (DeliveryFailure::Timeout, "timeout", FailureClass::Transient),
        (
            DeliveryFailure::RelayUnavailable,
            "relay_unavailable",
            FailureClass::Transient,
        ),
        (
            DeliveryFailure::ProtocolRejected,
            "protocol_rejected",
            FailureClass::Permanent,
        ),
        (
            DeliveryFailure::Unauthorised,
            "unauthorised",
            FailureClass::RetryableOnlyAfterUserAction,
        ),
        (
            DeliveryFailure::InvalidRecipientState,
            "invalid_recipient_state",
            FailureClass::RetryableOnlyAfterUserAction,
        ),
        (
            DeliveryFailure::MessageExpired,
            "message_expired",
            FailureClass::Permanent,
        ),
        (
            DeliveryFailure::ContactRevoked,
            "contact_revoked",
            FailureClass::RetryableOnlyAfterUserAction,
        ),
        (
            DeliveryFailure::PayloadTooLarge,
            "payload_too_large",
            FailureClass::Permanent,
        ),
        (
            DeliveryFailure::LocalStorageFailure,
            "local_storage_failure",
            FailureClass::Transient,
        ),
        (
            DeliveryFailure::InternalError,
            "internal_error",
            FailureClass::Transient,
        ),
    ];

    for (failure, code, class) in cases {
        assert_eq!(failure.code(), code);
        assert_eq!(failure.class(), class);
        assert_eq!(failure.to_string(), code);
    }
}

#[test]
fn diagnostic_detail_is_optional_and_redacted() {
    let without_detail = DeliveryError::new(DeliveryFailure::Timeout);
    assert_eq!(without_detail.code(), "timeout");
    assert_eq!(without_detail.detail(), None);

    let with_detail = DeliveryError::with_detail(
        DeliveryFailure::ConnectionFailed,
        "dial failed for secret-token",
    );
    assert_eq!(with_detail.code(), "connection_failed");
    assert_eq!(with_detail.detail(), Some("dial failed for secret-token"));
    assert_eq!(
        with_detail.to_string(),
        "connection_failed: dial failed for secret-token"
    );

    let sanitized = DeliveryError::with_detail(DeliveryFailure::InternalError, "  line\nfeed\t ");
    assert_eq!(sanitized.detail(), Some("line feed"));

    let oversized = DeliveryError::with_detail(DeliveryFailure::InternalError, "x".repeat(600));
    assert_eq!(oversized.detail().unwrap().len(), 512);
}
