//! Causal ordering of encrypted group messages via a lamport clock.
//!
//! Bridges boru's causal ordering requirements to the pattern expected by
//! p2panda-encryption's `ForwardSecureOrdering` trait but uses our local
//! [`EncryptedGroupEnvelope`](crate::group_encryption::message::EncryptedGroupEnvelope) and types to avoid depending on a concrete
//! [`AckedGroupMembership`](p2panda_encryption::traits::AckedGroupMembership)
//! at this level.
//!
//! # Lamport clock
//!
//! Each message is stamped with a lamport clock value (monotonically
//! increasing per-group counter).  Incoming messages are queued; a message
//! becomes "ready" when all of its explicitly-declared dependencies have been
//! processed.  The orderer tracks a DAG of dependency edges and yields
//! messages in causal order.
//!
//! # State
//!
//! [`OrderingState`](crate::group_encryption::ordering::OrderingState) holds:
//! - The per-group lamport clock (shared across all members).
//! - A dependency DAG (message → [`OpId`](crate::group_encryption::types::OpId) dependencies).
//! - All queued messages.
//! - A welcome-message anchor that marks the "create group" baseline.
//!
//! # Trait
//!
//! [`ForwardSecureOrdering`](crate::group_encryption::ordering::ForwardSecureOrdering) is a local trait (mirroring p2panda-encryption's
//! trait) that provides the ordering API used by the group encryption layer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use p2panda_encryption::message_scheme::{
    dcgka::DirectMessage as DcgkaDirectMessage, ControlMessage, Generation,
};
use p2panda_encryption::traits::ForwardSecureOrdering as P2pandaForwardSecureOrdering;
use serde::{Deserialize, Serialize};

use super::membership::Membership;
use super::message::{EncryptedGroupEnvelope, ForwardSecureGroupMessage};
use super::types::{OpId, PeerId};

// ── LamportClock ────────────────────────────────────────────────────────────

/// A lamport clock value used for causal ordering of group messages.
///
/// The clock is per-group and monotonically increasing.  Each outgoing
/// message is stamped with the current clock value; incoming messages update
/// the clock to `max(ours, theirs) + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LamportClock(pub u64);

impl LamportClock {
    /// Create a new lamport clock starting at the given value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Advance the clock to the next value.
    pub fn tick(&mut self) -> u64 {
        let val = self.0;
        self.0 += 1;
        val
    }

    /// Update this clock to `max(self, other) + 1`
    /// after observing a message with the given clock.
    pub fn observe(&mut self, other: LamportClock) {
        self.0 = self.0.max(other.0) + 1;
    }

    /// The current clock value.
    pub fn current(&self) -> u64 {
        self.0
    }
}

impl Default for LamportClock {
    fn default() -> Self {
        Self(1)
    }
}

// ── OrderingState ───────────────────────────────────────────────────────────

/// Per-group ordering state for the forward-secure ordering protocol.
///
/// Tracks:
/// - The lamport clock value (shared across all group members).
/// - A dependency DAG: each message declares dependencies
///   (the [`OpId`]s of messages it depends on).
/// - A ready queue of messages whose dependencies are satisfied.
/// - All queued messages are retained in memory for the DAG traversal.
/// - An optional welcome-message anchor.
///
/// This state is serialisable (via serde) for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderingState {
    /// Per-group lamport clock.
    clock: LamportClock,
    /// Our own peer identifier.
    my_id: PeerId,
    /// FIFO queue of message IDs whose dependencies are met.
    ready: VecDeque<OpId>,
    /// Map: dependency [`OpId`] → set of message IDs that depend on it.
    /// Used to cascade when a dependency becomes ready.
    dependents: HashMap<OpId, HashSet<OpId>>,
    /// Map: message [`OpId`] → its list of dependency [`OpId`]s.
    /// Needed to know when *all* dependencies are satisfied.
    deps_of: HashMap<OpId, Vec<OpId>>,
    /// All queued messages indexed by their [`OpId`].
    messages: HashMap<OpId, EncryptedGroupEnvelope>,
    /// The welcome message that established this ordering baseline.
    /// `None` if we haven't been welcomed yet.
    welcome_message: Option<EncryptedGroupEnvelope>,
}

impl OrderingState {
    /// Creates a new ordering state for a group.
    ///
    /// `my_id` is our own peer identifier.  The lamport clock starts at 1.
    pub fn new(my_id: PeerId) -> Self {
        Self {
            clock: LamportClock::default(),
            my_id,
            ready: VecDeque::new(),
            dependents: HashMap::new(),
            deps_of: HashMap::new(),
            messages: HashMap::new(),
            welcome_message: None,
        }
    }

    /// The current lamport clock value.
    pub fn clock(&self) -> LamportClock {
        self.clock
    }

    /// Returns `true` if we have been welcomed (via `set_welcome`).
    pub fn is_welcomed(&self) -> bool {
        self.welcome_message.is_some()
    }

    /// Number of messages currently in the pending (not-ready) queue.
    pub fn pending_count(&self) -> usize {
        self.deps_of.len().saturating_sub(self.ready.len())
    }

    /// Number of messages currently in the ready queue.
    pub fn ready_count(&self) -> usize {
        self.ready.len()
    }

    /// Check whether all dependencies for a given message are satisfied.
    fn all_deps_satisfied(&self, deps: &[OpId]) -> bool {
        deps.iter().all(|dep| self.is_processed(dep))
    }

    /// Returns true if the message with `id` is in the ready queue.
    fn is_ready(&self, id: &OpId) -> bool {
        self.ready.contains(id)
    }

    /// Returns true if the message with `id` has been, or is about to be,
    /// processed.  A message is considered "processed" if it is in the
    /// ready queue, is the welcome message (applied directly by the
    /// receive welcome branch, never via the ready queue), or if it has
    /// been fully consumed and cleaned up.
    fn is_processed(&self, id: &OpId) -> bool {
        if self.is_ready(id) {
            return true;
        }
        // The welcome message is processed directly by `process_ready`
        // when the group is established — `next_ready_message` skips it,
        // so it never enters the normal "consumed" cleanup path but must
        // still satisfy dependencies for later remote messages.
        if let Some(welcome) = &self.welcome_message {
            if welcome.id() == *id {
                return true;
            }
        }
        // A dependency is satisfied if it's in the ready queue or has
        // already been processed (no longer in deps_of or dependents).
        !self.deps_of.contains_key(id)
            && !self.dependents.contains_key(id)
            && !self.messages.contains_key(id)
    }
}

// ── ForwardSecureOrdering trait (local) ─────────────────────────────────────

/// Local ordering trait mirroring p2panda-encryption's
/// [`ForwardSecureOrdering`](p2panda_encryption::traits::ForwardSecureOrdering).
///
/// This trait avoids depending on a concrete
/// [`AckedGroupMembership`](p2panda_encryption::traits::AckedGroupMembership)
/// implementation and uses our local [`EncryptedGroupEnvelope`] and types.
pub trait ForwardSecureOrdering {
    /// Serializable ordering state (per-group).
    type State: Clone + std::fmt::Debug + Serialize + for<'a> Deserialize<'a>;

    /// Error type for ordering operations.
    type Error: std::error::Error;

    /// Create a control message with correct ordering metadata and return
    /// the updated state plus the new message.
    fn next_control_message(
        y: Self::State,
        control_message: &ControlMessage<PeerId, OpId>,
        direct_messages: &[Vec<u8>],
    ) -> Result<(Self::State, EncryptedGroupEnvelope), Self::Error>;

    /// Create an application message with correct ordering metadata
    /// and return the updated state plus the new message.
    fn next_application_message(
        y: Self::State,
        generation: Generation,
        ciphertext: Vec<u8>,
    ) -> Result<(Self::State, EncryptedGroupEnvelope), Self::Error>;

    /// Queue an incoming message for causal ordering.
    ///
    /// If the message's dependencies are satisfied it becomes ready
    /// immediately; otherwise it enters the pending queue.
    fn queue(y: Self::State, message: &EncryptedGroupEnvelope) -> Result<Self::State, Self::Error>;

    /// Mark a message as the welcome anchor.
    ///
    /// This is the "create group" or "add" message that established us
    /// as a group member.  Messages before this anchor are skipped
    /// on `next_ready_message`.
    fn set_welcome(
        y: Self::State,
        message: &EncryptedGroupEnvelope,
    ) -> Result<Self::State, Self::Error>;

    /// Pop the next ready message whose causal dependencies are satisfied.
    ///
    /// Returns `None` if no messages are ready.
    fn next_ready_message(
        y: Self::State,
    ) -> Result<(Self::State, Option<EncryptedGroupEnvelope>), Self::Error>;
}

// ── LamportOrderer ──────────────────────────────────────────────────────────

/// Unit struct implementing [`ForwardSecureOrdering`] using a lamport clock
/// plus explicit dependency tracking (DAG).
///
/// Internal ordering logic:
/// 1. Each outgoing message includes the current lamport clock value and
///    references the IDs of all previously-seen messages as dependencies.
/// 2. Incoming messages are enqueued via [`ForwardSecureOrdering::queue`]
///    which checks if all declared dependencies have already been processed.
/// 3. When all dependencies are satisfied, the message moves to the ready
///    queue and is yielded by [`ForwardSecureOrdering::next_ready_message`].
/// 4. The welcome anchor establishes a baseline: messages before it are
///    skipped.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LamportOrderer;

// ── Orderer error ───────────────────────────────────────────────────────────

/// Errors that can occur during ordering operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderingError {
    /// An ordering invariant was violated (e.g. self-dependency).
    InvariantViolation(String),
    /// A cycle was detected in the dependency graph.
    CycleDetected(OpId),
}

impl fmt::Display for OrderingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderingError::InvariantViolation(msg) => {
                write!(f, "ordering invariant violated: {msg}")
            }
            OrderingError::CycleDetected(id) => {
                write!(f, "dependency cycle detected for message {id:?}")
            }
        }
    }
}

impl std::error::Error for OrderingError {}

// ── Helper: extract dependencies from an EncryptedGroupEnvelope ──────────────

/// Extract the list of [`OpId`] dependencies for a given incoming message.
///
/// In this lamport-clock based implementation, messages depend on all
/// previously-seen messages (including our own) except the message being
/// queued itself.
///
/// Own messages are excluded: they were applied locally at creation time
/// via `process_local`/`Dcgka::process_local`, so remote messages never
/// need to wait on them (mirrors p2panda's reference orderer, which
/// filters `previous` to drop messages from `my_id`).
fn extract_dependencies(state: &OrderingState, exclude_id: &OpId) -> Vec<OpId> {
    state
        .messages
        .iter()
        .filter(|(id, msg)| *id != exclude_id && msg.sender() != state.my_id)
        .map(|(id, _)| *id)
        .collect()
}

// ── ForwardSecureOrdering implementation for LamportOrderer ─────────────────

impl ForwardSecureOrdering for LamportOrderer {
    type State = OrderingState;
    type Error = OrderingError;

    fn next_control_message(
        mut y: Self::State,
        control_message: &ControlMessage<PeerId, OpId>,
        direct_messages: &[Vec<u8>],
    ) -> Result<(Self::State, EncryptedGroupEnvelope), Self::Error> {
        // Advance the lamport clock.
        y.clock.tick();

        // Build the envelope (uses hash of content as ID).
        let envelope = EncryptedGroupEnvelope::new_control(
            y.my_id,
            control_message.clone(),
            direct_messages.to_vec(),
        );

        // Store for dependency tracking, but do NOT mark as ready: own
        // messages are applied locally by the caller (via Dcgka
        // `process_local`) and broadcast to peers — the `receive` loop
        // must never re-process them as remote messages (p2panda asserts
        // `sender != my_id` in `process_remote`).
        y.messages.insert(envelope.id(), envelope.clone());

        Ok((y, envelope))
    }

    fn next_application_message(
        mut y: Self::State,
        generation: Generation,
        ciphertext: Vec<u8>,
    ) -> Result<(Self::State, EncryptedGroupEnvelope), Self::Error> {
        // Advance the lamport clock.
        y.clock.tick();

        // Build the envelope.
        let envelope =
            EncryptedGroupEnvelope::new_application(y.my_id, ciphertext, generation, vec![]);

        // Store for dependency tracking only (see next_control_message).
        y.messages.insert(envelope.id(), envelope.clone());

        Ok((y, envelope))
    }

    fn queue(
        mut y: Self::State,
        message: &EncryptedGroupEnvelope,
    ) -> Result<Self::State, Self::Error> {
        let id = message.id();

        // Don't re-queue our own messages.
        if message.sender() == y.my_id {
            return Ok(y);
        }

        // Check for duplicate.
        if y.messages.contains_key(&id) {
            return Ok(y);
        }

        // Store the message.
        y.messages.insert(id, message.clone());

        // Extract dependencies: all previously-seen messages.
        let deps = extract_dependencies(&y, &id);

        if deps.is_empty() {
            // No dependencies — immediately ready.
            y.ready.push_back(id);
            return Ok(y);
        }

        // Check if all dependencies are satisfied.
        if y.all_deps_satisfied(&deps) {
            y.ready.push_back(id);

            // Cascade to any pending messages that depend on this one.
            y = cascade_ready(y, id)?;

            return Ok(y);
        }

        // Dependencies not yet satisfied — register as pending.
        y.deps_of.insert(id, deps.clone());

        // Register this message as a dependent of each dependency.
        for dep in &deps {
            y.dependents.entry(*dep).or_default().insert(id);
        }

        Ok(y)
    }

    fn set_welcome(
        mut y: Self::State,
        message: &EncryptedGroupEnvelope,
    ) -> Result<Self::State, Self::Error> {
        y.welcome_message = Some(message.clone());

        // Store the welcome message if not already stored.
        // (It may already be in the ready queue from next_control_message.)
        y.messages
            .entry(message.id())
            .or_insert_with(|| message.clone());

        Ok(y)
    }

    fn next_ready_message(
        mut y: Self::State,
    ) -> Result<(Self::State, Option<EncryptedGroupEnvelope>), Self::Error> {
        // If we haven't been welcomed yet, don't process any messages.
        let welcome_id = match &y.welcome_message {
            Some(welcome) => welcome.id(),
            None => return Ok((y, None)),
        };

        loop {
            // Peek at the next ready message.
            let next_id = match y.ready.pop_front() {
                Some(id) => id,
                None => return Ok((y, None)),
            };

            // Remove from pending tracking.
            y.deps_of.remove(&next_id);

            // If this is the welcome message itself, skip it (already
            // processed by set_welcome).
            if next_id == welcome_id {
                continue;
            }

            // Retrieve the message.
            let message = match y.messages.remove(&next_id) {
                Some(msg) => msg,
                None => {
                    // Message was already consumed — try next.
                    continue;
                }
            };

            // Cascade: any pending message that depended on this one
            // should check if it's now fully satisfied.
            y = cascade_ready(y, next_id)?;

            return Ok((y, Some(message)));
        }
    }
}

// ── p2panda-encryption ForwardSecureOrdering impl ─────────────────────────
//
// Bridges LamportOrderer into p2panda-encryption's ForwardSecureOrdering
// trait so that MessageGroup can use it directly.

impl P2pandaForwardSecureOrdering<PeerId, OpId, Membership> for LamportOrderer {
    type State = OrderingState;
    type Error = OrderingError;
    type Message = EncryptedGroupEnvelope;

    fn next_control_message(
        y: Self::State,
        control_message: &ControlMessage<PeerId, OpId>,
        direct_messages: &[DcgkaDirectMessage<PeerId, OpId, Membership>],
    ) -> Result<(Self::State, Self::Message), Self::Error> {
        // Serialize DirectMessage structs to opaque bytes for storage.
        let dm_bytes: Vec<Vec<u8>> = direct_messages
            .iter()
            .map(|dm| postcard::to_allocvec(dm).unwrap_or_default())
            .collect();
        <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            y,
            control_message,
            &dm_bytes,
        )
    }

    fn next_application_message(
        y: Self::State,
        generation: Generation,
        ciphertext: Vec<u8>,
    ) -> Result<(Self::State, Self::Message), Self::Error> {
        <LamportOrderer as ForwardSecureOrdering>::next_application_message(
            y, generation, ciphertext,
        )
    }

    fn queue(y: Self::State, message: &Self::Message) -> Result<Self::State, Self::Error> {
        <LamportOrderer as ForwardSecureOrdering>::queue(y, message)
    }

    fn set_welcome(y: Self::State, message: &Self::Message) -> Result<Self::State, Self::Error> {
        <LamportOrderer as ForwardSecureOrdering>::set_welcome(y, message)
    }

    fn next_ready_message(
        y: Self::State,
    ) -> Result<(Self::State, Option<Self::Message>), Self::Error> {
        <LamportOrderer as ForwardSecureOrdering>::next_ready_message(y)
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// After a message becomes ready, check all messages that depended on it
/// and promote them to ready if their dependencies are now satisfied.
fn cascade_ready(mut y: OrderingState, key: OpId) -> Result<OrderingState, OrderingError> {
    // Get all dependents of this key.
    let dependents = y.dependents.remove(&key).unwrap_or_default();

    for dep_id in dependents {
        // Get the remaining dependencies for this dependent.
        let deps = match y.deps_of.get(&dep_id) {
            Some(d) => d.clone(),
            None => continue,
        };

        // Check if all deps are now satisfied.
        if y.all_deps_satisfied(&deps) {
            y.deps_of.remove(&dep_id);
            y.ready.push_back(dep_id);

            // Recurse: this dependent becoming ready may unlock others.
            y = cascade_ready(y, dep_id)?;
        }
    }

    Ok(y)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: generate a PeerId for testing.
    fn make_peer() -> PeerId {
        let sk = iroh::SecretKey::generate();
        PeerId::from(sk.public())
    }

    /// Helper: create a control envelope with a given sender.
    fn make_control(sender: PeerId) -> EncryptedGroupEnvelope {
        EncryptedGroupEnvelope::new_control(
            sender,
            ControlMessage::Create {
                initial_members: vec![sender],
            },
            vec![],
        )
    }

    // ── LamportClock tests ────────────────────────────────────────────────

    #[test]
    fn test_lamport_clock_default_starts_at_one() {
        let clock = LamportClock::default();
        assert_eq!(clock.current(), 1);
    }

    #[test]
    fn test_lamport_clock_tick_increments() {
        let mut clock = LamportClock::new(5);
        assert_eq!(clock.tick(), 5);
        assert_eq!(clock.current(), 6);
        assert_eq!(clock.tick(), 6);
        assert_eq!(clock.current(), 7);
    }

    #[test]
    fn test_lamport_clock_observe_updates() {
        let mut clock = LamportClock::new(1);
        clock.observe(LamportClock::new(5));
        assert_eq!(clock.current(), 6);

        clock.observe(LamportClock::new(3));
        assert_eq!(clock.current(), 7);
    }

    #[test]
    fn test_lamport_clock_observe_lower_does_not_regress() {
        let mut clock = LamportClock::new(10);
        clock.observe(LamportClock::new(3));
        assert_eq!(clock.current(), 11);
    }

    // ── OrderingState tests ───────────────────────────────────────────────

    #[test]
    fn test_ordering_state_initial() {
        let peer = make_peer();
        let state = OrderingState::new(peer);
        assert_eq!(state.clock(), LamportClock::new(1));
        assert!(!state.is_welcomed());
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.ready_count(), 0);
    }

    // ── ForwardSecureOrdering tests ───────────────────────────────────────

    #[test]
    fn test_next_control_message_stamps_message() {
        let my_id = make_peer();
        let state = OrderingState::new(my_id);

        let (state, msg) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![my_id],
            },
            &[],
        )
        .expect("next_control_message");

        assert_eq!(msg.sender(), my_id, "sender should be our peer");
        // Own messages are stored for dependency tracking but never enter
        // the ready queue — they are applied locally by the caller and
        // broadcast to peers, so `next_ready_message` must not yield them.
        assert_eq!(
            state.ready_count(),
            0,
            "own control message must not be in the ready queue"
        );
        assert_eq!(state.messages.len(), 1, "own message stored for deps");
        assert_eq!(state.clock(), LamportClock::new(2), "clock should advance");
    }

    #[test]
    fn test_next_application_message_stamps_message() {
        let my_id = make_peer();
        let state = OrderingState::new(my_id);

        let (state, msg) = <LamportOrderer as ForwardSecureOrdering>::next_application_message(
            state,
            1,
            b"encrypted-data".to_vec(),
        )
        .expect("next_application_message");

        assert_eq!(msg.sender(), my_id, "sender should be our peer");
        assert_eq!(
            state.ready_count(),
            0,
            "own application message must not be in the ready queue"
        );
        assert_eq!(
            state.messages.len(),
            1,
            "own application message stored for deps"
        );
    }

    #[test]
    fn test_own_messages_do_not_enter_ready_queue() {
        // When we publish our own messages, they are stored for dependency
        // tracking but never yielded by `next_ready_message` — the receive
        // loop only processes remote messages (p2panda asserts
        // `sender != my_id` in `process_remote`).
        let my_id = make_peer();
        let state = OrderingState::new(my_id);

        let (state, create) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![my_id],
            },
            &[],
        )
        .expect("create");
        let state = <LamportOrderer as ForwardSecureOrdering>::set_welcome(state, &create)
            .expect("set_welcome");

        let (state, msg2) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Update,
            &[],
        )
        .expect("msg2");

        assert_eq!(state.ready_count(), 0, "own messages never queued");

        let (_, popped) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop");
        assert!(popped.is_none(), "no remote messages to process");
        let _ = msg2;
    }

    #[test]
    fn test_remote_messages_after_welcome_are_yielded() {
        // A remote (queued) message whose dependencies are met is yielded
        // by `next_ready_message`; own messages are not.
        let alice = make_peer();
        let bob = make_peer();
        let state = OrderingState::new(alice);

        let (state, create) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![alice, bob],
            },
            &[],
        )
        .expect("create");
        let state = <LamportOrderer as ForwardSecureOrdering>::set_welcome(state, &create)
            .expect("set_welcome");

        // Bob sends a message (remote) — it depends on the Create, which is
        // the welcome and therefore satisfied.
        let bob_msg = make_control(bob);
        let state = <LamportOrderer as ForwardSecureOrdering>::queue(state, &bob_msg)
            .expect("queue bob_msg");

        // Alice sends a second message (own) — stored, not queued.
        let (state, alice_msg) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Update,
            &[],
        )
        .expect("alice_msg");

        // Ready queue: [Create (welcome), bob_msg]
        // Pop 1: skip welcome, return bob_msg
        let (state, popped) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop");
        assert_eq!(popped, Some(bob_msg), "Bob's message ready first");

        // Pop 2: own message is NOT queued — nothing left.
        let (_, popped2) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop2");
        assert!(popped2.is_none(), "own message is not yielded");
        let _ = alice_msg;
    }

    #[test]
    fn test_set_welcome_establishes_ordering_baseline() {
        let my_id = make_peer();
        let alice = make_peer();
        let state = OrderingState::new(my_id);

        let (state, create_msg) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![my_id, alice],
            },
            &[],
        )
        .expect("create");

        let state = <LamportOrderer as ForwardSecureOrdering>::set_welcome(state, &create_msg)
            .expect("set_welcome");

        assert!(state.is_welcomed(), "should be welcomed");
        // The welcome (own create) is stored for deps but not queued.
        assert_eq!(state.ready_count(), 0, "own welcome not in ready queue");

        // Nothing remote to process — the welcome was applied directly by
        // the receive welcome branch, not via the ready queue.
        let (state, popped) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop");
        assert!(popped.is_none(), "no remote messages after welcome");

        let (_, popped2) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop2");
        assert!(popped2.is_none(), "no more messages");
    }

    #[test]
    fn test_queue_holds_messages_until_deps_met() {
        let alice = make_peer();
        let bob = make_peer();

        let state = OrderingState::new(alice);
        let (state, create) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![alice, bob],
            },
            &[],
        )
        .expect("create");
        let state = <LamportOrderer as ForwardSecureOrdering>::set_welcome(state, &create)
            .expect("set_welcome");

        // Bob sends a message (depends on nothing remote — own create is
        // excluded from deps, so Bob's message is ready immediately).
        let bob_msg = make_control(bob);
        let state =
            <LamportOrderer as ForwardSecureOrdering>::queue(state, &bob_msg).expect("queue");

        // Alice sends another message (own — stored, not queued).
        let (state, alice_msg) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Update,
            &[],
        )
        .expect("alice_msg");

        // Ready queue: [bob_msg] (create is own/welcome, not queued)
        // Pop 1: return bob_msg
        let (state, popped_bob) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop bob");
        assert_eq!(popped_bob, Some(bob_msg), "Bob's message first");

        // Pop 2: own message is not queued — nothing left.
        let (_, popped_alice) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state)
                .expect("pop alice");
        assert!(popped_alice.is_none(), "own message is not yielded");
        let _ = alice_msg;
    }

    #[test]
    fn test_multiple_peers_ordered_fifo() {
        let alice = make_peer();
        let bob = make_peer();
        let charlie = make_peer();

        let state = OrderingState::new(alice);
        let (state, create) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![alice, bob, charlie],
            },
            &[],
        )
        .expect("create");
        let state = <LamportOrderer as ForwardSecureOrdering>::set_welcome(state, &create)
            .expect("set_welcome");

        // Bob and Charlie both send messages (both depend on nothing remote
        // — the create is own/welcome, excluded from deps).
        let bob_msg = make_control(bob);
        let charlie_msg = make_control(charlie);

        let state = <LamportOrderer as ForwardSecureOrdering>::queue(state, &charlie_msg)
            .expect("queue charlie");
        let state =
            <LamportOrderer as ForwardSecureOrdering>::queue(state, &bob_msg).expect("queue bob");

        // Alice sends update (own — stored, not queued).
        let (state, alice_update) =
            <LamportOrderer as ForwardSecureOrdering>::next_control_message(
                state,
                &ControlMessage::Update,
                &[],
            )
            .expect("alice_update");

        // Ready queue: [charlie_msg, bob_msg]
        // Pop 1: charlie_msg (queued first)
        let (state, popped_c) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state)
                .expect("pop charlie");
        assert_eq!(popped_c, Some(charlie_msg), "Charlie first (FIFO)");

        // Pop 2: bob_msg
        let (state, popped_b) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop bob");
        assert_eq!(popped_b, Some(bob_msg), "Bob second");

        // Pop 3: own update is NOT queued — nothing left.
        let (_, popped_a) = <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state)
            .expect("pop alice");
        assert!(popped_a.is_none(), "own update is not yielded");
        let _ = alice_update;
    }

    #[test]
    fn test_ready_message_returns_none_when_not_welcomed() {
        let my_id = make_peer();
        let state = OrderingState::new(my_id);

        // Without a welcome call, next_ready_message should return None.
        let (_, popped) =
            <LamportOrderer as ForwardSecureOrdering>::next_ready_message(state).expect("pop");
        assert!(popped.is_none(), "no messages without welcome");
    }

    #[test]
    fn test_deduplicate_queue() {
        let alice = make_peer();
        let bob = make_peer();

        let state = OrderingState::new(alice);
        let (state, create) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![alice, bob],
            },
            &[],
        )
        .expect("create");
        let state = <LamportOrderer as ForwardSecureOrdering>::set_welcome(state, &create)
            .expect("set_welcome");

        let bob_msg = make_control(bob);
        let state =
            <LamportOrderer as ForwardSecureOrdering>::queue(state, &bob_msg).expect("queue first");

        // Queue the same message again should be a no-op.
        let state = <LamportOrderer as ForwardSecureOrdering>::queue(state, &bob_msg)
            .expect("queue duplicate");
        assert_eq!(state.messages.len(), 2, "no duplicate message stored");
    }

    #[test]
    fn test_own_messages_not_queued() {
        let my_id = make_peer();

        let state = OrderingState::new(my_id);
        let (state, create) = <LamportOrderer as ForwardSecureOrdering>::next_control_message(
            state,
            &ControlMessage::Create {
                initial_members: vec![my_id],
            },
            &[],
        )
        .expect("create");

        // Try to queue our own message — it must be a no-op (own messages
        // never enter the ready queue; they are stored by next_control_message).
        let state = <LamportOrderer as ForwardSecureOrdering>::queue(state, &create)
            .expect("queue own msg");
        assert_eq!(state.ready_count(), 0, "own message never queued");
        assert_eq!(state.messages.len(), 1, "own message stored exactly once");
    }
}
