use serde::Serialize;

pub const EMPTY_TICKET: &str = "empty_ticket";
pub const INVALID_TICKET: &str = "invalid_ticket";
pub const TICKET_NOT_AUTHORIZED: &str = "ticket_not_authorized";
pub const ALIAS_TOO_LONG: &str = "alias_too_long";
pub const INTERNAL: &str = "internal";

/// Safe, stable error payload for frontend IPC. Never put raw errors or secrets here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: &'static str,
}

impl CommandError {
    pub const fn internal() -> Self {
        Self {
            code: INTERNAL,
            message: "The command could not be completed.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_only_the_stable_camel_case_error_shape() {
        let value = serde_json::to_value(CommandError::internal()).unwrap();
        assert_eq!(value, serde_json::json!({
            "code": "internal",
            "message": "The command could not be completed."
        }));
    }
}
