//! Recovered regressions for bounded claims and worker shutdown.
use std::{
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use boru_core::{
    outbox_delivery::{AllowListedPolicy, BoxFuture, DeliveryTransport, OutboxDeliveryWorker},
    storage::Storage,
    store::StoredEnvelope,
};
use iroh::{PublicKey, SecretKey};
use tokio::sync::mpsc;

struct Transport;
impl DeliveryTransport for Transport {
    fn deliver(&self, _: PublicKey, _: StoredEnvelope) -> BoxFuture<n0_error::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test]
async fn concurrent_claim_budget_preserves_remaining_rows_and_leases() {
    for (messages, limit, batch) in [(5, 3, 5), (2, 3, 5), (3, 3, 3), (7, 3, 2), (2, 1, 0)] {
        let storage = Storage::memory().unwrap();
        let peer = SecretKey::generate().public();
        let now = now_ms();
        for i in 0..messages {
            let envelope = StoredEnvelope {
                msg_id: [i as u8; 32],
                conversation_id: [42; 32],
                author_user_id: peer,
                author_device_id: peer,
                created_at_ms: now,
                expires_at_ms: now + 86_400_000,
                ciphertext: bytes::Bytes::from_static(b"budget-regression"),
                signature: [0; 64],
                acked_at_ms: None,
            };
            storage.insert_inbox(&envelope).unwrap();
            storage.enqueue_outbox(&envelope.msg_id, peer, now).unwrap();
        }
        let (_trigger, rx) = mpsc::channel(1);
        let worker = OutboxDeliveryWorker::new(
            storage.clone(),
            Arc::new(AllowListedPolicy(|_| async { Ok(true) })),
            Arc::new(Transport),
            "historical-budget",
            rx,
        )
        .with_max_concurrent(NonZeroUsize::new(2).unwrap())
        .with_claim_limit(limit)
        .with_claim_batch_size(batch);
        let mut remaining = messages;
        while remaining > 0 {
            let attempted = tokio::time::timeout(Duration::from_secs(2), worker.run_once())
                .await
                .expect("run_once must return after its budget");
            assert_eq!(attempted, remaining.min(limit as usize));
            remaining -= attempted;
            let due = storage.fetch_due_outbox(now_ms()).unwrap();
            assert_eq!(due.len(), remaining, "unattempted rows must remain due");
            let claimable = storage
                .claim_n_due_outbox(now_ms(), "observer", 10_000, 32)
                .unwrap();
            assert_eq!(
                claimable.len(),
                remaining,
                "unattempted rows must not retain leases"
            );
            for row in claimable {
                storage
                    .release_outbox_lease(&row.msg_id, row.recipient_device_id, "observer")
                    .unwrap();
            }
        }
        assert_eq!(worker.run_once().await, 0);
    }
}

#[tokio::test]
async fn worker_exits_when_trigger_closes() {
    let (trigger, rx) = mpsc::channel(1);
    drop(trigger);
    let worker = OutboxDeliveryWorker::new(
        Storage::memory().unwrap(),
        Arc::new(AllowListedPolicy(|_| async { Ok(true) })),
        Arc::new(Transport),
        "historical-shutdown",
        rx,
    );
    tokio::time::timeout(Duration::from_secs(1), worker.run())
        .await
        .expect("closed trigger must stop the worker despite the timer");
}

#[tokio::test]
async fn reconnect_worker_exits_when_trigger_closes() {
    let (trigger, rx) = mpsc::channel(1);
    let (_reconnect_trigger, reconnects) = mpsc::channel(1);
    drop(trigger);
    let worker = OutboxDeliveryWorker::new(
        Storage::memory().unwrap(),
        Arc::new(AllowListedPolicy(|_| async { Ok(true) })),
        Arc::new(Transport),
        "historical-reconnect-shutdown",
        rx,
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        worker.run_with_reconnects(reconnects, 4),
    )
    .await
    .expect("closed trigger must stop the reconnect worker despite the timer");
}
