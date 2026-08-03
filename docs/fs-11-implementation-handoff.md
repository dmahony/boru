FS-11 "Peers Downloading from Me" — implementation handoff
==========================================================

This file preserves the complete FS-11 implementation that was present in the
working tree at ~04:30 AEST and subsequently deleted by concurrent sibling
workers (FS-09/FS-12/FS-13/FS-14/FS-15/FS-16) who were dispatched into the SAME
shared `dir` workspace (/home/dan/iroh-gossip-chat) and kept rewriting
app.rs/main.rs/dashboard_view_model.rs every few minutes.

The tree could not be verified/committed because:
  1. Other workers' half-done code blocks compilation independently of FS-11
     (missing `downloaded_history` field init, `LocalFileState`,
     `project_validated_remote_shared_file`, `refresh_downloaded_history` etc).
  2. Any FS-11 edit was clobbered within minutes by sibling rewrites.

Once the orchestrator serializes the Phase C/D cards (or gives FS-11 a scratch
workspace), apply the pieces below and run the verification steps at the end.

NOTE: the prior FS-11 attempt (run 3767) is where this code originated. Some
line numbers below refer to the pre-clobber state (~04:30); re-locate by
symbol name after re-applying.

====================================================================
FILE 1: src/transfer_state_projection.rs (additions — survived, still present)
====================================================================
After `fn terminal(self)` add:
    /// True for finished states that should leave the live dashboard list and
    /// move to activity history. Disconnected is intentionally not terminal —
    /// an interrupted transfer may resume when the peer returns.
    pub fn is_terminal(self) -> bool {
        self.terminal()
    }

After `pub fn snapshot(&self)` add (on TransferStateStore):
    /// Mark active transfers for an authenticated peer disconnected and
    /// broadcast every resulting update to subscribers.
    pub fn disconnect_peer(&self, peer_id: &str, occurred_at_ms: u64) {
        let updates = self
            .projection
            .lock()
            .expect("transfer projection lock")
            .disconnect_peer(peer_id, occurred_at_ms);
        for update in updates {
            let _ = self.updates.send(update);
        }
    }

====================================================================
FILE 2: examples/iced_chat/dashboard_view_model.rs (additions — deleted by FS-09 worker)
====================================================================
Add to the Progress enum impl (already has fraction()/from_bytes()):

/// Project an FS-05 outbound transfer record into a compact panel row.
///
/// The peer label is the authenticated peer id string from the projection —
/// the caller resolves it to a verified display identity; it is never read
/// from an untrusted display field. The file label is a UI enrichment keyed
/// by the stable item id (content hash) and falls back to a short hash
/// prefix rather than a fabricated name or local path.
pub(crate) fn outbound_row(
    record: &boru_core::transfer_state_projection::TransferRecord,
    item_labels: &std::collections::HashMap<String, String>,
) -> PeerDownload {
    let peer_id = record
        .peer_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let display_name = item_labels
        .get(&record.item_id)
        .cloned()
        .unwrap_or_else(|| {
            let prefix: String = record.item_id.chars().take(12).collect();
            format!("file {prefix}…")
        });
    let state = OutboundState::from(record.state);
    let state = if state == OutboundState::Transferring && record.attempt > 1 {
        OutboundState::Retrying
    } else {
        state
    };
    PeerDownload {
        id: StableId::new(format!("transfer:{}", record.transfer_id)),
        peer_id: StableId::new(format!("peer:{peer_id}")),
        peer_label: peer_id,
        file_id: StableId::new(format!("item:{}", record.item_id)),
        display_name,
        progress: Progress::from_bytes(record.bytes, record.total_bytes),
        updated_at_ms: record.updated_at_ms,
        state,
        error: record.error.clone(),
        attempt: record.attempt,
    }
}

/// Sort outbound rows newest-first with a stable id tiebreaker.
pub(crate) fn sort_outbound_rows(rows: &mut [PeerDownload]) {
    rows.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
}

Add to PeerDownload struct the fields:
    pub(crate) state: OutboundState,
    pub(crate) error: Option<String>,
    pub(crate) attempt: u32,

Add the OutboundState enum:

/// Dashboard-visible outbound transfer state, derived from the projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboundState {
    Transferring,
    Retrying,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Disconnected,
}

impl From<boru_core::transfer_state_projection::TransferState> for OutboundState {
    fn from(state: boru_core::transfer_state_projection::TransferState) -> Self {
        use boru_core::transfer_state_projection::TransferState;
        match state {
            TransferState::Active => Self::Transferring,
            TransferState::Verifying => Self::Verifying,
            TransferState::Completed => Self::Completed,
            TransferState::Failed => Self::Failed,
            TransferState::Cancelled => Self::Cancelled,
            TransferState::Disconnected => Self::Disconnected,
        }
    }
}

====================================================================
FILE 3: examples/iced_chat/app.rs
====================================================================
(a) Re-add import at top:
use boru_core::transfer_state_projection::{
    EventName, ProjectionUpdate, TransferDirection, TransferEvent, TransferRecord, TransferState,
    TransferStateStore, TransferUpdateReceiver,
};

(b) IcedChat struct fields:
    /// FS-05 live transfer projection store (source of the outbound panel).
    transfer_store: Arc<TransferStateStore>,
    /// item_id (content hash) → display name, filled by the outbound
    /// provider consumer; never a local path.
    outbound_item_labels: Arc<StdMutex<HashMap<String, String>>>,
    /// Active outbound transfer records by stable transfer id.
    outbound_active: HashMap<String, TransferRecord>,
    /// Recently finished outbound transfers (bounded history, newest first).
    outbound_history: VecDeque<TransferRecord>,

(c) Const: const MAX_OUTBOUND_HISTORY: usize = 50;

(d) IcedChat::new(): add params `transfer_store: Arc<TransferStateStore>`,
    `outbound_item_labels: Arc<StdMutex<HashMap<String, String>>>` and seed:
    let mut outbound_active: HashMap<String, TransferRecord> = HashMap::new();
    let mut outbound_history: VecDeque<TransferRecord> = VecDeque::new();
    for record in transfer_store.snapshot() {
        if record.direction != TransferDirection::Outbound { continue; }
        if record.state.is_terminal() {
            outbound_history.push_back(record);
        } else {
            outbound_active.insert(record.transfer_id.clone(), record);
        }
    }
    outbound_history.truncate(MAX_OUTBOUND_HISTORY);
    (assign fields in Self { ... })

(e) AppMessage variants:
    /// A reduced FS-05 projection update for an outbound transfer.
    TransferProjectionUpdate(ProjectionUpdate),
    /// The transfer broadcast receiver lagged or restarted; rebuild the panel
    /// maps from the projection snapshot.
    TransferSnapshotResync,

(f) update() handlers:
    AppMessage::TransferProjectionUpdate(update) => {
        self.apply_transfer_update(update.transfer);
        iced::Task::none()
    }
    AppMessage::TransferSnapshotResync => {
        let snapshot = self.transfer_store.snapshot();
        self.resync_outbound_panel(&snapshot);
        iced::Task::none()
    }

(g) Panel state machine (impl IcedChat):

    fn apply_transfer_update(&mut self, record: TransferRecord) {
        if record.direction != TransferDirection::Outbound { return; }
        if record.state.is_terminal() {
            if self.outbound_active.remove(&record.transfer_id).is_some()
                || !self.outbound_history.iter().any(|existing| existing.transfer_id == record.transfer_id)
            {
                self.outbound_history.push_front(record);
                self.outbound_history.truncate(MAX_OUTBOUND_HISTORY);
            }
        } else {
            self.outbound_history.retain(|existing| existing.transfer_id != record.transfer_id);
            self.outbound_active.insert(record.transfer_id.clone(), record);
        }
    }

    fn resync_outbound_panel(&mut self, snapshot: &[TransferRecord]) {
        self.outbound_active.clear();
        self.outbound_history.clear();
        for record in snapshot {
            if record.direction != TransferDirection::Outbound { continue; }
            if record.state.is_terminal() {
                self.outbound_history.push_back(record.clone());
            } else {
                self.outbound_active.insert(record.transfer_id.clone(), record.clone());
            }
        }
        let mut history: Vec<TransferRecord> = self.outbound_history.drain(..).collect();
        history.sort_by(|a, b| {
            b.updated_at_ms.cmp(&a.updated_at_ms)
                .then_with(|| a.transfer_id.cmp(&b.transfer_id))
        });
        history.truncate(MAX_OUTBOUND_HISTORY);
        self.outbound_history = history.into();
    }

(h) view_peers_downloading_from_me(&self, theme: &iced::Theme) — renders
    CardShell::new("Peers Downloading from Me", children)
        .count(active_count)
        .on_view_all(AppMessage::DashboardTabSelected(DashboardTab::Downloading))
        .empty_message("No one is downloading from you right now.")
        .max_height(240.0)
        .build(theme)
    Rows: outbound_active.values() -> outbound_row(record, &labels),
    sorted with sort_outbound_rows; each row rendered by peer_download_row.

(i) peer_download_row(&self, row: &PeerDownload, theme: &iced::Theme):
    - peer label: row.peer_label.parse::<PublicKey>() -> self.resolve_name(&pk)
      else "Unknown peer"; online dot from self.peer_presence.
    - state label/color mapping for Transferring/Retrying/Verifying/Completed/
      Failed/Cancelled/Disconnected (design_tokens primary/warning/success/
      danger/muted).
    - progress: Determinate -> ProgressBar::new(pct/100).bold().build(theme) +
      "{pct}%"; Indeterminate -> ProgressBar::new(0.0).indeterminate(true).bold()
      .build(theme) + "{bytes} received"; Unknown -> empty bar + "—".
    - Avatar::new(&peer_display).size(28.0).online_dot(online)...
    NOTE: use .indeterminate(true) (takes bool), .color_fn(crate::design_tokens::text_muted)
    (fn pointer, NOT closure capturing theme), .wrapping(iced::widget::text::Wrapping::None)
    (no Ellipsis variant in this iced).

(j) In view_file_sharing replace placeholder right-rail card with:
    let peers_section = self.view_peers_downloading_from_me(&theme);
    (call sites pass &theme, not theme)

(k) subscription: IcedChat::subscription() gains a `transfer_store: Arc<TransferStateStore>`
    param, builds a transfer branch via TransferStoreHandle (broadcast receiver);
    on RecvError::Lagged -> AppMessage::TransferSnapshotResync, Closed -> disable.

====================================================================
FILE 4: examples/iced_chat/main.rs
====================================================================
(a) At top of imports add:
use iroh_blobs::{
    provider::events::{ConnectMode, EventMask, EventSender, ProviderMessage, RequestMode},
    store::fs::FsStore,
    BlobsProtocol,
};

(b) Before the `let (...)` tuple from runtime.block_on, create shared stores:
    let transfer_store = std::sync::Arc::new(
        boru_core::transfer_state_projection::TransferStateStore::new(256),
    );
    let outbound_item_labels: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

(c) Inside the async block, after blob_store load:
    let (blob_event_sender, blob_event_rx) = EventSender::channel(
        128,
        EventMask {
            connected: ConnectMode::Notify,
            get: RequestMode::NotifyLog,
            get_many: RequestMode::NotifyLog,
            push: RequestMode::Disabled,
            observe: iroh_blobs::provider::events::ObserveMode::None,
            throttle: iroh_blobs::provider::events::ThrottleMode::None,
        },
    );
    spawn_outbound_provider_consumer(
        runtime.handle(),
        blob_event_rx,
        Arc::clone(&transfer_store),
        Arc::clone(&outbound_item_labels),
        Some(storage.clone()),
        local_public,
    );
    let blobs_protocol = BlobsProtocol::new(&blob_store, Some(blob_event_sender));

(d) Pass transfer_store + outbound_item_labels into IcedChat::new(...)
    (order: ..., Arc::clone(&tunnel_service), transfer_store, outbound_item_labels).

(e) Add the consumer function (full body):

fn spawn_outbound_provider_consumer(
    runtime: &tokio::runtime::Handle,
    mut rx: tokio::sync::mpsc::Receiver<ProviderMessage>,
    store: Arc<boru_core::transfer_state_projection::TransferStateStore>,
    item_labels: Arc<Mutex<std::collections::HashMap<String, String>>>,
    storage: Option<Arc<Storage>>,
    local_public: PublicKey,
) {
    use boru_core::transfer_state_projection::{EventName, TransferDirection, TransferEvent};
    use iroh_blobs::provider::events::RequestUpdate;
    runtime.spawn(async move {
        let mut peers: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
        let mut transfers: std::collections::HashMap<(u64, u64), String> =
            std::collections::HashMap::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                ProviderMessage::ClientConnected(msg)
                | ProviderMessage::ClientConnectedNotify(msg) => {
                    if let Some(peer) = msg.inner.endpoint_id {
                        peers.insert(msg.inner.connection_id, peer.to_string());
                    }
                }
                ProviderMessage::ConnectionClosed(msg) => {
                    if let Some(peer) = peers.get(&msg.inner.connection_id).cloned() {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        store.disconnect_peer(&peer, now_ms);
                    }
                    peers.remove(&msg.inner.connection_id);
                    transfers.retain(|(conn, _), _| *conn != msg.inner.connection_id);
                }
                ProviderMessage::GetRequestReceivedNotify(msg)
                | ProviderMessage::GetManyRequestReceivedNotify(msg) => {
                    let key = (msg.inner.connection_id, msg.inner.request_id);
                    let transfer_id = transfers.entry(key).or_insert_with(|| {
                        format!("serve:{}-{}", msg.inner.connection_id, msg.inner.request_id)
                    }).clone();
                    let peer_id = peers.get(&msg.inner.connection_id).cloned();
                    let update_rx = msg.rx;
                    let store = store.clone();
                    let item_labels = item_labels.clone();
                    let storage = storage.clone();
                    runtime.spawn(async move {
                        let mut sequence = 0u64;
                        let mut current_hash: Option<String> = None;
                        while let Ok(Some(update)) = update_rx.recv().await {
                            sequence += 1;
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            let event = match update {
                                RequestUpdate::Started(started) => {
                                    let hash_hex = started.hash.to_string();
                                    if let Ok(mut labels) = item_labels.lock() {
                                        labels.entry(hash_hex.clone()).or_insert_with(|| {
                                            storage.as_ref().and_then(|stg| {
                                                stg.get_shared_file(&local_public.to_string(), &hash_hex).ok().flatten()
                                            }).map(|row| row.display_filename).unwrap_or_else(|| {
                                                let prefix: String = hash_hex.chars().take(12).collect();
                                                format!("file {prefix}…")
                                            })
                                        });
                                    }
                                    current_hash = Some(hash_hex.clone());
                                    TransferEvent {
                                        event_id: format!("serve:{transfer_id}:started:{sequence}:{now_ms}"),
                                        transfer_id: transfer_id.clone(),
                                        item_id: hash_hex,
                                        direction: TransferDirection::Outbound,
                                        peer_id: peer_id.clone(),
                                        sequence,
                                        attempt: 1,
                                        occurred_at_ms: now_ms,
                                        kind: EventName::Started,
                                        bytes: 0,
                                        total_bytes: Some(started.size),
                                        error: None,
                                    }
                                }
                                RequestUpdate::Progress(progress) => TransferEvent {
                                    event_id: format!("serve:{transfer_id}:progress:{sequence}:{now_ms}"),
                                    transfer_id: transfer_id.clone(),
                                    item_id: current_hash.clone().unwrap_or_default(),
                                    direction: TransferDirection::Outbound,
                                    peer_id: peer_id.clone(),
                                    sequence,
                                    attempt: 1,
                                    occurred_at_ms: now_ms,
                                    kind: EventName::Progress,
                                    bytes: progress.end_offset,
                                    total_bytes: None,
                                    error: None,
                                },
                                RequestUpdate::Completed(completed) => TransferEvent {
                                    event_id: format!("serve:{transfer_id}:completed:{sequence}:{now_ms}"),
                                    transfer_id: transfer_id.clone(),
                                    item_id: current_hash.clone().unwrap_or_default(),
                                    direction: TransferDirection::Outbound,
                                    peer_id: peer_id.clone(),
                                    sequence,
                                    attempt: 1,
                                    occurred_at_ms: now_ms,
                                    kind: EventName::Completed,
                                    bytes: completed.stats.payload_bytes_sent,
                                    total_bytes: None,
                                    error: None,
                                },
                                RequestUpdate::Aborted(_) => TransferEvent {
                                    event_id: format!("serve:{transfer_id}:aborted:{sequence}:{now_ms}"),
                                    transfer_id: transfer_id.clone(),
                                    item_id: current_hash.clone().unwrap_or_default(),
                                    direction: TransferDirection::Outbound,
                                    peer_id: peer_id.clone(),
                                    sequence,
                                    attempt: 1,
                                    occurred_at_ms: now_ms,
                                    kind: EventName::Failed,
                                    bytes: 0,
                                    total_bytes: None,
                                    error: Some("Transfer aborted before completion".to_string()),
                                },
                            };
                            store.publish(event);
                        }
                    });
                }
                _ => {}
            }
        }
    });
}

====================================================================
TESTS (examples/iced_chat/app.rs, tests module) — previously fixed to real API
====================================================================
Two tests: outbound_panel_keeps_active_rows_and_archives_terminal_rows_exactly_once
and outbound_snapshot_resync_rebuilds_live_rows_from_the_projection. They call
apply_transfer_update(TransferRecord) directly (NOT ProjectionUpdate::Upsert),
assert outbound_history.front().unwrap().state == TransferState::Completed, use
TransferEvent { event_id: String, kind: EventName, sequence, attempt, direction },
and check item_id / peer_id.as_deref().

====================================================================
VERIFICATION (after re-applying, when tree compiles)
====================================================================
1. cargo check --example boru --features gui        # expect 0 errors
2. cargo test --example boru outbound_panel --features gui    # 2 FS-11 tests
3. cargo test --features net transfer_state_projection::tests --lib   # 6+ tests
4. Manual: two/three-peer download, watch panel appear/complete/archive;
   disconnect peer -> Disconnected rows; reconnect -> retry row.
5. Commit message: "feat(FS-11): live Peers Downloading from Me panel wired to FS-05 outbound projection"
