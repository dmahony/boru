#![allow(missing_docs)]

use crate::store::{MessageStore, StoredEnvelope};
use iroh::{Endpoint, PublicKey};
use n0_error::Result;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

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

        if let Ok(due) = self.store.fetch_due_outbox(now_ms) {
            for row in due {
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
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            let _ = self.store.record_attempt(
                                &row.msg_id,
                                row.recipient_device_id,
                                now_ms + backoff_ms(row.attempts + 1),
                                Some(&err_str),
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
