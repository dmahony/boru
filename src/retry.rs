#![allow(missing_docs)]

use crate::diagnostics::{DIAGNOSTICS, DiagnosticEventKind};
use crate::store::{MessageStore, StoredEnvelope};
use iroh::{Endpoint, PublicKey};
use n0_error::Result;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Semaphore, mpsc};

use crate::abuse_controls::{MAX_CONCURRENT_DELIVERY_ATTEMPTS, MAX_RETRY_QUEUE_SIZE};

static DELIVERY_LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub trait InboxSender: Send + Sync {
    fn send_deliver<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        peer: PublicKey,
        env: StoredEnvelope,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;
}

#[derive(Debug)]
pub struct RetryWorker {
    store: MessageStore,
    endpoint: Endpoint,
    sender: Arc<dyn InboxSender>,
    trigger: mpsc::Receiver<()>,
}

impl std::fmt::Debug for dyn InboxSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InboxSender")
    }
}

impl RetryWorker {
    pub fn new(
        store: MessageStore,
        endpoint: Endpoint,
        sender: Arc<dyn InboxSender>,
        trigger: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            store,
            endpoint,
            sender,
            trigger,
        }
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                Some(_) = self.trigger.recv() => {
                    self.process_due().await;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                    self.process_due().await;
                }
                else => { break; } // Channel closed
            }
        }
    }

    async fn process_due(&self) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let _ = self.store.expire_outbox(now_ms);

        if let Ok(mut due) = self.store.fetch_due_outbox(now_ms) {
            // A corrupt or maliciously large queue must not spawn unbounded
            // delivery work in one tick.
            due.truncate(MAX_RETRY_QUEUE_SIZE);
            let limiter = DELIVERY_LIMIT
                .get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_DELIVERY_ATTEMPTS)));
            for row in due {
                let Ok(_permit) = limiter.clone().try_acquire_owned() else {
                    break;
                };
                let peer_id = row.recipient_device_id.to_string();
                let msg_short = hex::encode(&row.msg_id[..4]);
                let attempt_count = row.attempts;
                DIAGNOSTICS.record_with_peer(
                    None,
                    Some(&peer_id),
                    DiagnosticEventKind::OutboxClaimed {
                        message_id_short: Some(msg_short.clone()),
                        conversation_id_prefix: None,
                        peer_id: Some(peer_id.clone()),
                        attempt_count,
                        delivery_state: format!("{:?}", row.status),
                    },
                );
                DIAGNOSTICS.record_with_peer(
                    None,
                    Some(&peer_id),
                    DiagnosticEventKind::DeliveryAttemptStarted {
                        message_id_short: Some(msg_short.clone()),
                        conversation_id_prefix: None,
                        peer_id: Some(peer_id.clone()),
                        attempt_count,
                        retry_delay_ms: Some(row.next_attempt_at_ms.saturating_sub(now_ms)),
                    },
                );

                if let Ok(Some(env)) = self.store.get_inbox(&row.msg_id) {
                    match self
                        .sender
                        .send_deliver(&self.endpoint, row.recipient_device_id, env)
                        .await
                    {
                        Ok(_) => {
                            // If successful we record attempt. The actual ACK comes via ALPN handler later,
                            // but we mark the next retry window in case ACK drops.
                            let _ = self.store.record_attempt(
                                &row.msg_id,
                                row.recipient_device_id,
                                now_ms + backoff_ms(row.attempts + 1),
                                None,
                            );
                            DIAGNOSTICS.record_with_peer(
                                None,
                                Some(&peer_id.clone()),
                                DiagnosticEventKind::EnvelopeSent {
                                    message_id_short: Some(msg_short),
                                    conversation_id_prefix: None,
                                    peer_id: Some(peer_id.clone()),
                                    attempt_count,
                                    delivery_state: "SentAwaitingAck".to_string(),
                                    elapsed_ms: None,
                                },
                            );
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            let delay = backoff_ms(row.attempts + 1);
                            let _ = self.store.record_attempt(
                                &row.msg_id,
                                row.recipient_device_id,
                                now_ms + delay,
                                Some(&err_str),
                            );
                            let category = if err_str.contains("timeout")
                                || err_str.contains("Connection")
                            {
                                "connection".to_string()
                            } else if err_str.contains("reject") || err_str.contains("unauthorized")
                            {
                                "rejected".to_string()
                            } else if err_str.contains("expir") {
                                "expired".to_string()
                            } else {
                                "transient".to_string()
                            };
                            DIAGNOSTICS.record_with_peer(
                                None,
                                Some(&peer_id.clone()),
                                DiagnosticEventKind::RetryScheduled {
                                    message_id_short: Some(msg_short),
                                    conversation_id_prefix: None,
                                    peer_id: Some(peer_id.clone()),
                                    attempt_count,
                                    retry_delay_ms: delay,
                                    failure_category: category,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

fn backoff_ms(attempts: u32) -> u64 {
    match attempts {
        0 => 5_000,
        1 => 30_000,
        2 => 120_000,
        3 => 600_000,
        4 => 1_800_000,
        5 => 7_200_000,
        _ => 21_600_000, // max 6h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MessageId;
    use bytes::Bytes;

    // MockSender for testing
    #[allow(dead_code)]
    struct MockSender {
        success: bool,
    }

    impl InboxSender for MockSender {
        fn send_deliver<'a>(
            &'a self,
            _endpoint: &'a Endpoint,
            _peer: PublicKey,
            _env: StoredEnvelope,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            let success = self.success;
            Box::pin(async move {
                if success {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("mock error").into())
                }
            })
        }
    }
}
