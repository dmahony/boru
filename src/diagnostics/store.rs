//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

// =============================================================================
// Diagnostics store (core type)
// =============================================================================

/// Thread-safe diagnostics store with bounded event and probe buffers.
///
/// # Defaults
///
/// | Store              | Max entries |
/// |--------------------|-------------|
/// | Events             | 5 000       |
/// | Received probes    | 1 000       |
///
/// When a store exceeds its maximum, the oldest entries are evicted at
/// the next insert.  Query limits are clamped to 1 000.
///
/// When the `net` feature is enabled, a Tokio watch channel is available
/// for event-driven waiting (see [`Diagnostics::subscribe`]).
#[derive(Debug, Clone)]
pub struct Diagnostics {
    inner: Arc<DiagnosticsInner>,
}

#[derive(Debug)]
struct DiagnosticsInner {
    /// Bounded event ring buffer.  Newest entries are appended at the back.
    events: Mutex<VecDeque<DiagnosticEvent>>,
    /// Bounded received-probe map keyed by opaque identifier string.
    /// Insertion order is tracked via a parallel deque for eviction.
    received_probes: Mutex<HashMap<String, ReceivedProbe>>,
    /// Insertion-order queue for received probes (keys, oldest first).
    received_probe_order: Mutex<VecDeque<String>>,
    /// Monotonically increasing sequence counter.
    next_sequence: AtomicU64,
    /// Maximum event storage capacity.
    max_events: usize,
    /// Maximum received-probe storage capacity.
    max_received_probes: usize,
    /// Tokio watch sender for event notifications (net feature only).
    #[cfg(feature = "net")]
    event_watch: tokio::sync::watch::Sender<u64>,
}

impl Diagnostics {
    /// Create a new diagnostics store with default capacities.
    ///
    /// - Events: 5 000
    /// - Received probes: 1 000
    pub fn new() -> Self {
        Self::with_capacity(5000, 1000)
    }

    /// Create a new diagnostics store with the given capacities.
    pub fn with_capacity(max_events: usize, max_received_probes: usize) -> Self {
        Self {
            inner: Arc::new(DiagnosticsInner {
                events: Mutex::new(VecDeque::with_capacity(max_events.min(5000) + 64)),
                received_probes: Mutex::new(HashMap::with_capacity(
                    max_received_probes.min(1000) + 64,
                )),
                received_probe_order: Mutex::new(VecDeque::with_capacity(
                    max_received_probes.min(1000) + 64,
                )),
                next_sequence: AtomicU64::new(0),
                max_events,
                max_received_probes,
                #[cfg(feature = "net")]
                event_watch: tokio::sync::watch::Sender::new(0),
            }),
        }
    }

    /// Record a new diagnostic event.
    ///
    /// The event is assigned the next sequence number and a current
    /// timestamp automatically.  If the event store is at capacity,
    /// the oldest event is evicted.
    pub fn record(&self, room_id: Option<TopicId>, kind: DiagnosticEventKind) {
        self.record_with_peer(room_id, None::<&str>, kind);
    }

    /// Record a new diagnostic event with an optional peer ID.
    pub fn record_with_peer(
        &self,
        room_id: Option<TopicId>,
        peer_id: Option<impl AsRef<str>>,
        kind: DiagnosticEventKind,
    ) {
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        let event = DiagnosticEvent {
            sequence,
            timestamp: Utc::now(),
            room_id,
            peer_id: peer_id.map(|p| super::safety::redact_endpoint(p.as_ref())),
            kind: super::safety::sanitize_event_kind(kind),
        };

        {
            let mut events = self.inner.events.lock().expect("events lock");
            if events.len() >= self.inner.max_events {
                events.pop_front();
            }
            events.push_back(event);
        }

        #[cfg(feature = "net")]
        {
            let _ = self.inner.event_watch.send(sequence);
        }
    }

    /// Return events with a sequence number greater than `since_sequence`,
    /// limited to `limit` entries and optionally filtered by `room_id`.
    ///
    /// The limit is clamped to 1 000.  Events are returned in ascending
    /// sequence order (oldest matching first).
    pub fn events_since(
        &self,
        since_sequence: u64,
        limit: usize,
        room_id: Option<TopicId>,
    ) -> Vec<DiagnosticEvent> {
        let limit = limit.min(1000);
        let events = self.inner.events.lock().expect("events lock");

        let iter: Box<dyn Iterator<Item = &DiagnosticEvent>> = if let Some(room) = room_id {
            Box::new(events.iter().filter(move |e| e.room_id == Some(room)))
        } else {
            Box::new(events.iter())
        };

        iter.filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return events with a sequence number greater than `since_sequence`,
    /// optionally filtered by both `room_id` and `peer_id`.
    pub fn events_since_filtered(
        &self,
        since_sequence: u64,
        limit: usize,
        room_id: Option<TopicId>,
        peer_id: Option<&str>,
    ) -> Vec<DiagnosticEvent> {
        let limit = limit.min(1000);
        let events = self.inner.events.lock().expect("events lock");

        let iter: Box<dyn Iterator<Item = &DiagnosticEvent>> = if let Some(room) = room_id {
            Box::new(events.iter().filter(move |e| e.room_id == Some(room)))
        } else {
            Box::new(events.iter())
        };

        let iter = if let Some(pid) = peer_id {
            let pid_owned = pid.to_string();
            Box::new(iter.filter(move |e| e.peer_id.as_deref() == Some(&pid_owned)))
        } else {
            iter
        };

        iter.filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return the most recently assigned sequence number.
    ///
    /// Returns 0 if no events have been recorded yet.
    pub fn latest_sequence(&self) -> u64 {
        let val = self.inner.next_sequence.load(Ordering::Relaxed);
        if val == 0 {
            0
        } else {
            val - 1
        }
    }

    /// Subscribe to new event notifications via Tokio watch.
    ///
    /// The watch sends the latest sequence number each time an event is
    /// recorded.  Use this to implement event-driven waiting without
    /// aggressive polling.
    #[cfg(feature = "net")]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.event_watch.subscribe()
    }

    /// Record a received probe.
    ///
    /// This is the enhanced version that stores full probe metadata.
    /// If a probe with the same `probe_id` already exists, its
    /// `duplicate_count` is incremented.
    pub fn record_received_probe_enhanced(&self, probe: ReceivedProbe) {
        let id = probe.probe_id.clone();
        let mut probes = self.inner.received_probes.lock().expect("probes lock");
        let mut order = self
            .inner
            .received_probe_order
            .lock()
            .expect("probe order lock");

        // If already exists, increment duplicate count and replace
        if let Some(existing) = probes.get_mut(&id) {
            existing.duplicate_count += 1;
            existing.received_at_ms = probe.received_at_ms;
            existing.latency_ms = probe.latency_ms;
            // Refresh position in order
            if let Some(pos) = order.iter().position(|k| k == &id) {
                order.remove(pos);
            }
            order.push_back(id.clone());
            return;
        }

        // Evict oldest if at capacity
        if probes.len() >= self.inner.max_received_probes {
            if let Some(oldest_key) = order.pop_front() {
                probes.remove(&oldest_key);
            }
        }

        probes.insert(id.clone(), probe);
        order.push_back(id);
    }

    /// Record a received probe (legacy API, simple keyed storage).
    ///
    /// * `id` — opaque probe identifier.
    /// * `peer` — public key of the sending peer.
    /// * `discovery_source` — how the peer was discovered.
    /// * `room_id` — optional room context.
    pub fn record_received_probe(
        &self,
        id: String,
        peer: PublicKey,
        _discovery_source: DiscoverySource,
        room_id: Option<TopicId>,
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let probe = ReceivedProbe {
            probe_id: id.clone(),
            room_id: String::new(),
            sender_id: peer.to_string(),
            sent_at_ms: now_ms,
            received_at_ms: now_ms,
            latency_ms: None,
            message_hash: String::new(),
            duplicate_count: 0,
            timestamp: Utc::now(),
            room_id_opt: room_id,
        };

        let mut probes = self.inner.received_probes.lock().expect("probes lock");
        let mut order = self
            .inner
            .received_probe_order
            .lock()
            .expect("probe order lock");

        if let Some(pos) = order.iter().position(|k| k == &id) {
            order.remove(pos);
        }

        if probes.len() >= self.inner.max_received_probes {
            if let Some(oldest_key) = order.pop_front() {
                probes.remove(&oldest_key);
            }
        }

        probes.insert(id.clone(), probe);
        order.push_back(id);
    }

    /// Look up a received probe by its opaque identifier.
    pub fn find_received_probe(&self, id: &str) -> Option<ReceivedProbe> {
        let probes = self.inner.received_probes.lock().expect("probes lock");
        probes.get(id).cloned()
    }

    // ── Convenience helpers ──────────────────────────────────────────────

    /// Return the total number of events currently stored.
    pub fn event_count(&self) -> usize {
        let events = self.inner.events.lock().expect("events lock");
        events.len()
    }

    /// Return the next event sequence number (the raw atomic counter).
    /// Useful for precise event capture: pass this value to
    /// `events_since` / `events_since_filtered` with `>` semantics
    /// to capture events that have not yet been recorded.
    pub fn next_event_sequence(&self) -> u64 {
        self.inner.next_sequence.load(Ordering::Relaxed)
    }

    /// Remove all stored events and reset the event sequence counter.
    /// Intended for test use (parallel test isolation).
    pub fn reset_events(&self) {
        let mut events = self.inner.events.lock().expect("events lock");
        events.clear();
        self.inner.next_sequence.store(0, Ordering::Release);
    }

    /// Return the total number of received probes currently stored.
    pub fn probe_count(&self) -> usize {
        let probes = self.inner.received_probes.lock().expect("probes lock");
        probes.len()
    }

    /// Return all stored events (for diagnostics / debug).
    pub fn all_events(&self) -> Vec<DiagnosticEvent> {
        let events = self.inner.events.lock().expect("events lock");
        events.iter().cloned().collect()
    }

    /// Return unique topic IDs that have a RoomJoined event recorded.
    pub fn joined_rooms(&self) -> Vec<TopicId> {
        let events = self.inner.events.lock().expect("events lock");
        let mut rooms: HashSet<TopicId> = HashSet::new();
        for event in events.iter() {
            if matches!(event.kind, DiagnosticEventKind::RoomJoined) {
                if let Some(room_id) = event.room_id {
                    rooms.insert(room_id);
                }
            }
        }
        rooms.into_iter().collect()
    }

    /// Build a [`DiscoveryTestEvidence`] from the stored events.
    ///
    /// Scans all events for the given room and peer to determine which
    /// stages were reached.
    pub fn build_evidence(
        &self,
        room_id: Option<TopicId>,
        peer_id: Option<&str>,
    ) -> DiscoveryTestEvidence {
        let events = self.inner.events.lock().expect("events lock");

        let mut evidence = DiscoveryTestEvidence {
            local_room_joined: false,
            peer_discovered: false,
            address_lookup_observed: false,
            address_resolved: false,
            connection_attempted: false,
            connection_established: false,
            subscription_started: false,
            subscription_joined: false,
            peer_in_topic: false,
            probe_broadcast: false,
            probe_received_or_acknowledged: false,
        };

        for event in events.iter() {
            // Filter by room if specified
            if let Some(rid) = room_id {
                if event.room_id != Some(rid) {
                    continue;
                }
            }
            // Filter by peer if specified
            if let Some(pid) = peer_id {
                if event.peer_id.as_deref() != Some(pid) {
                    continue;
                }
            }

            match &event.kind {
                DiagnosticEventKind::RoomJoined => evidence.local_room_joined = true,
                DiagnosticEventKind::PeerDiscovered
                | DiagnosticEventKind::PeerDiscoveredWithAddr { .. } => {
                    evidence.peer_discovered = true;
                }
                DiagnosticEventKind::AddressLookupStarted { .. } => {
                    evidence.address_lookup_observed = true;
                }
                DiagnosticEventKind::AddressResolved { .. } => {
                    evidence.address_resolved = true;
                }
                DiagnosticEventKind::ConnectionAttemptStarted { .. } => {
                    evidence.connection_attempted = true;
                }
                DiagnosticEventKind::ConnectionEstablished { .. } => {
                    evidence.connection_established = true;
                }
                DiagnosticEventKind::RoomSubscriptionStarted => {
                    evidence.subscription_started = true;
                }
                DiagnosticEventKind::RoomSubscriptionJoined => {
                    evidence.subscription_joined = true;
                }
                DiagnosticEventKind::PeerAddedToTopic => {
                    evidence.peer_in_topic = true;
                }
                DiagnosticEventKind::ProbeBroadcast { .. } => {
                    evidence.probe_broadcast = true;
                }
                DiagnosticEventKind::ProbeReceived { .. } => {
                    evidence.probe_received_or_acknowledged = true;
                }
                _ => {}
            }
        }

        evidence
    }

    /// Rebuild per-peer diagnostic state from all stored events.
    ///
    /// Returns a map of peer_id → [`PeerDiagnosticState`] with the
    /// accumulated state for each observed peer.
    pub fn peer_states(&self) -> HashMap<String, PeerDiagnosticState> {
        let events = self.inner.events.lock().expect("events lock");
        let mut states: HashMap<String, PeerDiagnosticState> = HashMap::new();

        for event in events.iter() {
            if let Some(pid) = &event.peer_id {
                let current = states.remove(pid);
                let updated = update_peer_state(current, event);
                states.insert(pid.clone(), updated);
            }
        }

        states
    }

    /// Get the diagnostic state for a specific peer.
    pub fn peer_state(&self, peer_id: &str) -> Option<PeerDiagnosticState> {
        let events = self.inner.events.lock().expect("events lock");
        let mut state: Option<PeerDiagnosticState> = None;

        for event in events.iter() {
            if event.peer_id.as_deref() == Some(peer_id) {
                state = Some(update_peer_state(state, event));
            }
        }

        state
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::new()
    }
}
