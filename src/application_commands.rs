//! Typed application command boundary for frontend adapters.
//!
//! Mutations enter through this module rather than being represented as direct
//! UI update calls. Transport delivery remains owned by the existing runtime.
use crate::application_service::{
    ConversationApplicationService, PreparedTextMessage, TextSendRequest,
};
use crate::threads::ThreadTarget;

/// Stable machine-readable command failure category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandErrorCode {
    /// Required command input was missing or invalid.
    InvalidInput,
    /// The durable store rejected the mutation or the same idempotency key
    /// was reused for a different request.
    Conflict,
    /// A persistence or signing operation failed.
    Unavailable,
}

/// Safe error returned to a caller; detail contains no request or secret data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    /// Stable code suitable for client branching.
    pub code: CommandErrorCode,
    /// Human-readable, non-sensitive explanation.
    pub message: &'static str,
}

/// Explicit result of admitting a text message.
#[derive(Debug, Clone)]
pub enum CommandOutcome<T> {
    /// The mutation was durably admitted (possibly as a prior idempotent retry).
    Applied(T),
    /// Validation or execution failed with a stable error category.
    Rejected(CommandError),
}

/// A UI-independent text-send command.
#[derive(Debug, Clone)]
pub struct SendMessage {
    /// Caller-generated stable retry key; reuse only for retries of this request.
    pub idempotency_key: String,
    /// 32-byte conversation/topic identity.
    pub conversation_id: [u8; 32],
    /// Message body.
    pub text: String,
    /// Optional thread parent.
    pub thread_target: Option<ThreadTarget>,
}

/// Domain command facade. Keep transport and Iced state outside this boundary.
#[derive(Debug, Clone)]
pub struct ApplicationCommands {
    conversations: ConversationApplicationService,
}

impl ApplicationCommands {
    /// Build the command facade from the existing conversation application service.
    pub fn new(conversations: ConversationApplicationService) -> Self {
        Self { conversations }
    }

    /// Validate and durably admit one outgoing text message.
    pub fn send_message(&self, command: SendMessage) -> CommandOutcome<PreparedTextMessage> {
        if command.idempotency_key.trim().is_empty() {
            return CommandOutcome::Rejected(CommandError {
                code: CommandErrorCode::InvalidInput,
                message: "idempotency key must not be empty",
            });
        }
        if command.text.trim().is_empty() {
            return CommandOutcome::Rejected(CommandError {
                code: CommandErrorCode::InvalidInput,
                message: "message text must not be empty",
            });
        }
        match self.conversations.send_text(TextSendRequest {
            command_key: command.idempotency_key,
            conversation_id: command.conversation_id,
            text: command.text,
            thread_target: command.thread_target,
        }) {
            Ok(result) => CommandOutcome::Applied(result),
            Err(error) => {
                // Log only a stable category. Underlying errors may contain
                // filesystem paths or other implementation details.
                let error_text = format!("{error:?}").to_ascii_lowercase();
                let code = if error_text.contains("reused for a different request") {
                    CommandErrorCode::Conflict
                } else {
                    CommandErrorCode::Unavailable
                };
                tracing::warn!(?code, "application send_message rejected");
                CommandOutcome::Rejected(CommandError {
                    code,
                    message: match code {
                        CommandErrorCode::Conflict => {
                            "idempotency key conflicts with prior command"
                        }
                        CommandErrorCode::Unavailable => "message could not be admitted",
                        CommandErrorCode::InvalidInput => unreachable!(),
                    },
                })
            }
        }
    }

    /// Read-only query: conversation metadata. No UI state is mutated.
    pub fn conversation_meta(
        &self,
        conversation_id: &[u8; 32],
    ) -> n0_error::Result<Option<crate::store::ConversationMeta>> {
        self.conversations.conversation_meta(conversation_id)
    }

    /// Read-only query: unread count. No UI state is mutated.
    pub fn unread_count(&self, conversation_id: &[u8; 32]) -> n0_error::Result<Option<u32>> {
        self.conversations.unread_count(conversation_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application_service::ConversationApplicationService, storage::Storage, store::MessageStore,
    };
    use iroh::SecretKey;

    fn commands() -> ApplicationCommands {
        let key = SecretKey::from_bytes(&[17; 32]);
        ApplicationCommands::new(ConversationApplicationService::new(
            MessageStore::memory().unwrap(),
            Storage::memory().unwrap(),
            key.clone(),
            key.public(),
        ))
    }

    fn request(key: &str, text: &str) -> SendMessage {
        SendMessage {
            idempotency_key: key.into(),
            conversation_id: [0x42; 32],
            text: text.into(),
            thread_target: None,
        }
    }

    #[test]
    fn rejects_invalid_input_with_stable_code() {
        let result = commands().send_message(request(" ", "hello"));
        assert!(matches!(
            result,
            CommandOutcome::Rejected(CommandError {
                code: CommandErrorCode::InvalidInput,
                ..
            })
        ));
    }

    #[test]
    fn command_retry_returns_same_durable_message() {
        let commands = commands();
        let first = commands.send_message(request("retry-key", "hello"));
        let retry = commands.send_message(request("retry-key", "hello"));
        let (CommandOutcome::Applied(first), CommandOutcome::Applied(retry)) = (first, retry)
        else {
            panic!("valid command should be applied")
        };
        assert_eq!(first.event_id, retry.event_id);
        assert_eq!(first.encoded, retry.encoded);
    }

    #[test]
    fn key_reuse_for_different_content_is_a_typed_conflict() {
        let commands = commands();
        assert!(matches!(
            commands.send_message(request("same-key", "one")),
            CommandOutcome::Applied(_)
        ));
        let result = commands.send_message(request("same-key", "two"));
        assert!(
            matches!(
                result,
                CommandOutcome::Rejected(CommandError {
                    code: CommandErrorCode::Conflict,
                    ..
                })
            ),
            "{result:?}"
        );
    }
}
