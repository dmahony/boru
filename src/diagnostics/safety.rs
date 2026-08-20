//! Redaction helpers for lifecycle diagnostics.
//!
//! Diagnostics may describe failures, but must not become a second data
//! channel. These helpers intentionally prefer a coarse placeholder over
//! retaining a value that might contain an address, secret, path, or payload.

use super::DiagnosticEventKind;

/// Redact a value that may contain a network endpoint.
pub fn redact_endpoint(value: &str) -> String {
    if looks_like_ip_endpoint(value) {
        "<redacted-address>".to_string()
    } else {
        value.to_string()
    }
}

/// Redact error text when it contains credentials, paths, or payload markers.
/// Benign bounded error labels remain useful to operators.
pub fn redact_error(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let sensitive = [
        "authorization",
        "bearer ",
        "token=",
        "password",
        "secret",
        "private key",
        "file contents",
        "message body",
        "payload=",
        "clipboard",
        "/home/",
        "\\users\\",
    ];
    if sensitive.iter().any(|marker| lower.contains(marker)) || contains_ip_endpoint(value) {
        "<redacted-error>".to_string()
    } else {
        value.chars().take(160).collect()
    }
}

/// Replace arbitrary command or payload text with a safe presence marker.
pub fn redact_payload(_value: &str) -> String {
    "<redacted-payload>".to_string()
}

/// Sanitize fields in the event variants that can carry raw addresses,
/// failure internals, command JSON, or file/message payloads.
pub(crate) fn sanitize_event_kind(kind: DiagnosticEventKind) -> DiagnosticEventKind {
    match kind {
        DiagnosticEventKind::PeerDiscoveredWithAddr { source, addresses } => {
            DiagnosticEventKind::PeerDiscoveredWithAddr {
                source,
                addresses: addresses
                    .iter()
                    .map(|value| redact_endpoint(value))
                    .collect(),
            }
        }
        DiagnosticEventKind::AddressResolved { source, addresses } => {
            DiagnosticEventKind::AddressResolved {
                source,
                addresses: addresses
                    .iter()
                    .map(|value| redact_endpoint(value))
                    .collect(),
            }
        }
        DiagnosticEventKind::AddressLookupFailed { source, error } => {
            DiagnosticEventKind::AddressLookupFailed {
                source,
                error: redact_error(&error),
            }
        }
        DiagnosticEventKind::ConnectionAttemptStarted { addresses } => {
            DiagnosticEventKind::ConnectionAttemptStarted {
                addresses: addresses
                    .iter()
                    .map(|value| redact_endpoint(value))
                    .collect(),
            }
        }
        DiagnosticEventKind::ConnectionEstablished {
            remote_address,
            transport,
            used_relay,
        } => DiagnosticEventKind::ConnectionEstablished {
            remote_address: remote_address.as_deref().map(redact_endpoint),
            transport,
            used_relay,
        },
        DiagnosticEventKind::ConnectionFailed { addresses, error } => {
            DiagnosticEventKind::ConnectionFailed {
                addresses: addresses
                    .iter()
                    .map(|value| redact_endpoint(value))
                    .collect(),
                error: redact_error(&error),
            }
        }
        DiagnosticEventKind::RoomSubscriptionFailed { error } => {
            DiagnosticEventKind::RoomSubscriptionFailed {
                error: redact_error(&error),
            }
        }
        DiagnosticEventKind::Error(error) => DiagnosticEventKind::Error(redact_error(&error)),
        DiagnosticEventKind::GuiActionReceived {
            action_id,
            command_json,
        } => DiagnosticEventKind::GuiActionReceived {
            action_id,
            command_json: redact_payload(&command_json),
        },
        DiagnosticEventKind::BlobTransferFailed { transfer_id, error } => {
            DiagnosticEventKind::BlobTransferFailed {
                transfer_id,
                error: redact_error(&error),
            }
        }
        other => other,
    }
}

fn looks_like_ip_endpoint(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    if let Some((host, port)) = trimmed.rsplit_once(':') {
        let host = host.trim_matches(['[', ']']);
        return port.parse::<u16>().is_ok()
            && (host.parse::<std::net::IpAddr>().is_ok() || host == "localhost");
    }
    false
}

fn contains_ip_endpoint(value: &str) -> bool {
    value
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | ';' | '(' | ')' | '[' | ']')
        })
        .any(|part| looks_like_ip_endpoint(part.trim_matches([':', '.', '"', '\''])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_addresses_and_sensitive_errors() {
        assert_eq!(redact_endpoint("127.0.0.1:443"), "<redacted-address>");
        assert_eq!(redact_endpoint("[::1]:443"), "<redacted-address>");
        assert_eq!(
            redact_error("request failed: token=abc"),
            "<redacted-error>"
        );
        assert_eq!(redact_error("DNS timeout"), "DNS timeout");
    }

    #[test]
    fn payloads_are_never_retained() {
        assert_eq!(redact_payload("{\"body\":\"hello\"}"), "<redacted-payload>");
    }
}
