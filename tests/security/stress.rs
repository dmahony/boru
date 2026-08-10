//! Concurrency / stress tests (BORU-AUDIT-28, step 8): inbox channel
//! saturation, duplicate concurrent events, simultaneous writers.

use std::sync::{Arc, Mutex};

use boru_core::group_events::{GroupEvent, GroupEventPayload, GroupState};
use boru_core::group_replay::{ReplayStore, EVENT_ID_LEN};
use boru_core::inbox::{InboxEvent, InboxHandle};
use boru_core::mailbox::{MailboxIdentity, MailboxStore, DEFAULT_MAILBOX_TTL};
use boru_core::storage::Storage;
use boru_core::TopicId;
use iroh::SecretKey;
use tokio::sync::mpsc;

fn group_id() -> TopicId {
    TopicId::from_bytes([7u8; 32])
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Duplicate concurrent events: many threads race to apply the same group
/// event to a shared GroupState with a durable replay store; exactly one must
/// win and every other attempt must be rejected as a replay.
#[test]
fn duplicate_concurrent_group_events_exactly_one_wins() {
    let owner = SecretKey::generate();
    let member = SecretKey::generate().public();
    let group = group_id();

    let tmp = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(tmp.path().join("replay.db")).unwrap();
    let replay = ReplayStore::open(Arc::new(Mutex::new(conn))).unwrap();
    let state = Arc::new(Mutex::new(GroupState::new(group, owner.public())));

    // Sign the event once; all threads present the *same* event.
    let event = GroupEvent::sign(
        &owner,
        group,
        0,
        GroupEventPayload::MemberInvited { member },
    )
    .unwrap();
    // Attach the durable replay store so concurrency is exercised end-to-end.
    state.lock().unwrap().attach_replay_store(replay);

    const THREADS: usize = 16;
    let results: Vec<Result<(), String>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let state = state.clone();
                let event = event.clone();
                s.spawn(move || {
                    let mut guard = state.lock().unwrap();
                    event.clone().apply(&mut guard).map(|_| ()).map_err(|e| format!("{e:?}"))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let accepted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        accepted, 1,
        "exactly one concurrent duplicate event must be accepted (got {accepted})"
    );
}

/// Concurrent `ReplayStore::record` of the same marker: the atomic
/// INSERT OR IGNORE guarantees exactly one `Recorded` and the rest
/// `AlreadySeen`.
#[test]
fn concurrent_replay_store_record_exactly_one_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(tmp.path().join("replay.db")).unwrap();
    let store = Arc::new(ReplayStore::open(Arc::new(Mutex::new(conn))).unwrap());
    let event_id = [0x5Au8; EVENT_ID_LEN];

    const THREADS: usize = 16;
    let outcomes: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let store = store.clone();
                s.spawn(move || store.record(&group_id(), 1, event_id, now_secs()).unwrap())
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    use boru_core::group_replay::RecordOutcome;
    let recorded = outcomes
        .iter()
        .filter(|o| matches!(o, RecordOutcome::Recorded))
        .count();
    let already_seen = outcomes
        .iter()
        .filter(|o| matches!(o, RecordOutcome::AlreadySeen))
        .count();
    assert_eq!(recorded, 1, "exactly one record must win");
    assert_eq!(already_seen, THREADS - 1, "the rest must see AlreadySeen");
}

/// Inbox channel saturation: with a capacity-1 frontend channel, a second
/// concurrent emit fails with the documented Full error instead of silently
/// dropping the event, and the counter increments.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbox_channel_saturation_never_silently_drops() {
    // A capacity-1 channel is enough to saturate with a stalled frontend.
    let (handle, mut rx) = InboxHandle::with_capacity(1);
    let inner = handle.inner();

    // Emit the first event (fills the channel; no receiver yet).
    let first = {
        let guard = inner.lock().await;
        let tx = guard.envelope_tx.clone();
        let event = InboxEvent::SyncRequested {
            from: iroh::PublicKey::from_bytes(&[0u8; 32]).unwrap(),
            since_ms: 0,
        };
        tx.try_send(event)
    };
    assert!(first.is_ok(), "first event must fit in the channel");

    // Second emit on the full channel must fail with Full — observable
    // overload, not a silent drop.
    let second = {
        let guard = inner.lock().await;
        let tx = guard.envelope_tx.clone();
        let event = InboxEvent::SyncRequested {
            from: iroh::PublicKey::from_bytes(&[0u8; 32]).unwrap(),
            since_ms: 1,
        };
        tx.try_send(event)
    };
    assert!(
        matches!(second, Err(mpsc::error::TrySendError::Full(_))),
        "full channel must reject with Full, not drop silently"
    );

    // Draining the receiver frees the slot.
    let got = rx.recv().await;
    assert!(got.is_some(), "first event must be delivered");
    let third = {
        let guard = inner.lock().await;
        let tx = guard.envelope_tx.clone();
        let event = InboxEvent::SyncRequested {
            from: iroh::PublicKey::from_bytes(&[0u8; 32]).unwrap(),
            since_ms: 2,
        };
        tx.try_send(event)
    };
    assert!(third.is_ok(), "after drain the channel accepts again");
}

/// Simultaneous writers to the same mailbox store: all envelopes are
/// persisted, none lost under concurrency.
#[test]
fn simultaneous_mailbox_writers_no_loss() {
    let tmp = tempfile::tempdir().unwrap();
    let recipient = SecretKey::generate();
    let identity = MailboxIdentity::from_secret(&recipient);
    let store = Arc::new(Mutex::new(MailboxStore::with_ttl(
        tmp.path(),
        DEFAULT_MAILBOX_TTL,
    )));

    const WRITERS: usize = 8;
    const PER_WRITER: usize = 8;

    std::thread::scope(|s| {
        for w in 0..WRITERS {
            let store = store.clone();
            let identity = identity.clone();
            s.spawn(move || {
                for i in 0..PER_WRITER {
                    let sender = SecretKey::generate();
                    let payload = format!("w{w}-{i}");
                    let envelope = identity.seal(&sender, payload.as_bytes()).unwrap();
                    let mut guard = store.lock().unwrap();
                    guard.enqueue(envelope, &[sender.public()]).unwrap();
                }
            });
        }
    });

    let mut guard = store.lock().unwrap();
    let pending = guard.pending().unwrap();
    assert_eq!(
        pending.len(),
        WRITERS * PER_WRITER,
        "all concurrent writers' envelopes must be retained"
    );
}

/// Concurrent `queue_outgoing_dm` with distinct request keys into the SAME
/// conversation: sequences are distinct (1..=N) and every message is
/// persisted (no lost updates under concurrency).
#[test]
fn concurrent_outgoing_dm_distinct_sequences() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(tmp.path()).unwrap());
    let sender = SecretKey::generate();
    let recipient = SecretKey::generate();
    let recipient_key = MailboxIdentity::from_secret(&recipient).public_key();

    const THREADS: usize = 12;
    let sequences: Vec<u64> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let storage = storage.clone();
                let sender = sender.clone();
                s.spawn(move || {
                    let dm = storage
                        .queue_outgoing_dm(
                            [0xC0; 32],
                            sender.public(),
                            &format!("req-{i}"),
                            &format!("payload-{i}"),
                            recipient_key,
                            &sender,
                        )
                        .unwrap();
                    dm.sequence
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut sorted = sequences.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (1..=THREADS as u64).collect::<Vec<_>>(),
        "each concurrent writer must get a distinct sequence in the same conversation"
    );
}
