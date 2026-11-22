pub mod dto;

use serde::Serialize;

/// Minimal read-only settings projection exposed by the desktop bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsSummary {
    pub schema_version: u32,
    pub theme: Option<String>,
}

/// Error returned by bridge operations in a stable, frontend-friendly shape.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Invalid parameter {field}: {reason}")]
    InvalidParam { field: String, reason: String },
    #[error("{entity} not found: {id}")]
    NotFound { entity: String, id: String },
    #[error("Database error: {0}")]
    Db(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    code: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'a str>,
}

impl AppError {
    /// Produce the stable JSON envelope consumed by React command wrappers.
    pub fn to_json(&self) -> String {
        let (code, message, field) = match self {
            Self::InvalidParam { field, reason } => (
                "INVALID_PARAM",
                format!("Invalid parameter {field}: {reason}"),
                Some(field.as_str()),
            ),
            Self::NotFound { entity, id } => {
                ("NOT_FOUND", format!("{entity} not found: {id}"), None)
            }
            Self::Db(message) => ("DB_ERROR", format!("Database error: {message}"), None),
            Self::Internal(message) => ("INTERNAL", format!("Internal error: {message}"), None),
        };
        serde_json::to_string(&ErrorEnvelope {
            code,
            message,
            field,
        })
        .expect("serializing an error envelope cannot fail")
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value: serde_json::Value =
            serde_json::from_str(&self.to_json()).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

/// Validate an integer limit and return it unchanged on success.
pub fn validate_limit(value: i64, min: i64, max: i64) -> Result<i64, AppError> {
    if min > max || value < min || value > max {
        return Err(AppError::InvalidParam {
            field: "limit".into(),
            reason: format!("must be between {min} and {max}"),
        });
    }
    Ok(value)
}

/// Validate an opaque cursor's basic wire constraints.
pub fn validate_cursor(cursor: &str) -> Result<(), AppError> {
    if cursor.is_empty() || cursor.len() > 4096 || cursor.chars().any(char::is_control) {
        return Err(AppError::InvalidParam {
            field: "cursor".into(),
            reason: "must be a non-empty cursor of at most 4096 bytes".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn round_trip<T>(value: T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), value);
    }

    #[test]
    fn bridge_facade_exposes_the_canonical_contract_types() {
        use dto::*;
        assert_eq!(SNAPSHOT_SCHEMA_VERSION, 1);
        round_trip(MessageDto {
            id: "m".into(),
            conversation_id: "c".into(),
            sender_id: "s".into(),
            timestamp_ms: 1,
            kind: "text".into(),
            body: "hello".into(),
            delivery_status: Some(DeliveryStatusDto::Sent),
            attachment_handle: None,
        });
        round_trip(MessageHistoryPage {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            items: vec![MessageHistoryItem {
                id: "1".into(),
                conversation_id: "c".into(),
                sender_id: "s".into(),
                timestamp_ms: 2,
                kind: "text".into(),
                body: "hi".into(),
            }],
            next_cursor: Some(MessageHistoryCursor::new("mh1.ab:cd".into())),
        });
        round_trip(ContactDto {
            id: "x".into(),
            display_name: "X".into(),
            status: ContactStatusDto::Accepted,
        });
        round_trip(NetworkHealthSnapshot {
            ready: true,
            status: "connected".into(),
            connected_peer_count: Some(2),
            relay_url: None,
        });
    }

    #[test]
    fn validation_helpers_reject_out_of_range_input() {
        assert_eq!(validate_limit(50, 1, 200).unwrap(), 50);
        assert!(matches!(
            validate_limit(0, 1, 200),
            Err(AppError::InvalidParam { .. })
        ));
        assert!(matches!(
            validate_limit(201, 1, 200),
            Err(AppError::InvalidParam { .. })
        ));
        assert!(validate_cursor("mh1.payload").is_ok());
        assert!(matches!(
            validate_cursor(""),
            Err(AppError::InvalidParam { .. })
        ));
        let long = "a".repeat(4097);
        assert!(matches!(
            validate_cursor(&long),
            Err(AppError::InvalidParam { .. })
        ));
        assert!(matches!(
            validate_cursor("bad\ncursor"),
            Err(AppError::InvalidParam { .. })
        ));
    }

    #[test]
    fn errors_serialize_to_stable_envelopes() {
        let invalid = serde_json::to_value(AppError::InvalidParam {
            field: "limit".into(),
            reason: "out of range".into(),
        })
        .unwrap();
        assert_eq!(
            invalid,
            serde_json::json!({"code":"INVALID_PARAM","message":"Invalid parameter limit: out of range","field":"limit"})
        );
        let missing = serde_json::to_value(AppError::NotFound {
            entity: "conversation".into(),
            id: "c1".into(),
        })
        .unwrap();
        assert_eq!(
            missing,
            serde_json::json!({"code":"NOT_FOUND","message":"conversation not found: c1"})
        );
        assert!(missing.get("field").is_none());
    }
}
