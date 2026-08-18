//! Boru diagnostics submodule (structural split BORU-CORE-002).

use super::*;

// =============================================================================
// GUI Action Tracking
// =============================================================================

/// Deterministic error codes for structured GUI action errors.
///
/// Each variant encodes a specific failure condition that can occur during
/// action validation or processing.  This replaces unstructured error strings
/// with machine-readable codes that callers can handle programmatically.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum GuiActionErrorCode {
    /// GUI test actions are disabled (not started with --enable-gui-test-actions).
    GuiActionsDisabled,
    /// The specified room does not exist or has not been joined.
    UnknownRoom,
    /// The specified conversation does not exist.
    UnknownConversation,
    /// The specified peer is not known.
    UnknownPeer,
    /// The action is not valid for the current screen.
    InvalidCurrentScreen,
    /// A blocking dialog (e.g. confirmation modal) is open.
    BlockingDialogOpen,
    /// CloseDialog was requested while no application dialog was open.
    NoDialog,
    /// No active conversation to perform the action on.
    NoActiveConversation,
    /// The composer is empty (nothing to send).
    ComposerEmpty,
    /// The composer text exceeds the maximum allowed length.
    ComposerTooLong,
    /// Sending messages is currently disabled.
    SendDisabled,
    /// The room is inactive (left or disconnected).
    RoomInactive,
    /// The action queue has been closed (application shutting down).
    ActionQueueClosed,
    /// The action queue is full (at capacity).
    ActionQueueFull,
    /// The action timed out before completion.
    ActionTimedOut,
    /// An argument or parameter was invalid.
    InvalidArgument,
    /// The command could not be deserialized or was unrecognized.
    UnknownCommand,
    /// An internal system error occurred.
    InternalError,
}

/// A structured GUI action error with a deterministic error code and
/// human-readable message.
///
/// # Example
///
/// ```ignore
/// let err = GuiActionError::new(GuiActionErrorCode::UnknownRoom, "Room 'abc123' was not found");
/// assert_eq!(err.code, GuiActionErrorCode::UnknownRoom);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiActionError {
    /// The deterministic error code.
    pub code: GuiActionErrorCode,
    /// Human-readable explanation of the error.
    pub message: String,
}

impl GuiActionError {
    /// Create a new structured action error.
    pub fn new(code: GuiActionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for GuiActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for GuiActionError {}

/// A unique identifier for a GUI action.
///
/// Generated from a blake3 hash of the current timestamp, process ID, and
/// a random component, producing a 32-character hex string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GuiActionId(pub String);

impl GuiActionId {
    /// Generate a new unique action ID.
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut buf = [0u8; 16];
        let pid = std::process::id();
        let rnd: u64 = rand::random();
        let hash_input = format!("{now:020x}-{pid:x}-{rnd:016x}-gui-action");
        let hash = blake3::hash(hash_input.as_bytes());
        buf.copy_from_slice(&hash.as_bytes()[..16]);
        GuiActionId(hex::encode(buf))
    }
}

impl Default for GuiActionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GuiActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The expected UI state condition for a GUI action.
///
/// Each action can optionally declare what state it expects the UI to
/// be in after the action completes successfully.  The application state
/// checker uses this to verify that the action had the intended effect.
///
/// # Examples
///
/// ```
/// use boru_core::diagnostics::ExpectedState;
///
/// let state = ExpectedState::ScreenIs("chat_list".into());
/// assert!(state.matches_str("screen", "chat_list"));
/// assert!(!state.matches_str("screen", "settings"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ExpectedState {
    /// The active screen matches the given name (e.g. `"chat_list"`, `"settings"`, `"chat"`).
    ScreenIs(String),
    /// A room with the given topic hex string is selected / active.
    RoomSelected(String),
    /// A conversation with the given peer key (hex) is selected.
    ConversationSelected(String),
    /// The composer text matches the given string.
    ComposerTextIs(String),
    /// Dark mode matches the given boolean state.
    DarkModeIs(bool),
    /// A message was successfully submitted (send handled + composer cleared).
    MessageSent,
    /// The help overlay visibility matches the given boolean.
    HelpVisible(bool),
    /// Generic condition decribed by a free-form string.
    Generic(String),
}

impl ExpectedState {
    /// Check whether this expected state is satisfied by a given
    /// (category, value) observation from the UI.
    ///
    /// `category` is a string like `"screen"`, `"composer_text"`,
    /// `"dark_mode"`, etc.  `value` is the observed value.
    ///
    /// Returns `true` if the observation matches this expected state.
    pub fn matches_str(&self, category: &str, value: &str) -> bool {
        match self {
            ExpectedState::ScreenIs(expected) => category == "screen" && value == expected,
            ExpectedState::RoomSelected(expected) => category == "room" && value == expected,
            ExpectedState::ConversationSelected(expected) => {
                category == "conversation" && value == expected
            }
            ExpectedState::ComposerTextIs(expected) => {
                category == "composer_text" && value == expected
            }
            ExpectedState::DarkModeIs(expected) => {
                category == "dark_mode" && value == expected.to_string()
            }
            ExpectedState::MessageSent => category == "message_sent" && value == "true",
            ExpectedState::HelpVisible(expected) => {
                category == "help_visible" && value == expected.to_string()
            }
            ExpectedState::Generic(_) => false,
        }
    }

    /// Return a human-readable description of what condition this expected
    /// state represents (e.g. `"screen == chat_list"`).
    pub fn description(&self) -> String {
        match self {
            ExpectedState::ScreenIs(s) => format!("screen == \"{s}\""),
            ExpectedState::RoomSelected(t) => format!("room_selected({t})"),
            ExpectedState::ConversationSelected(k) => format!("conversation_selected({k})"),
            ExpectedState::ComposerTextIs(t) => format!("composer_text == \"{t}\""),
            ExpectedState::DarkModeIs(b) => format!("dark_mode == {b}"),
            ExpectedState::MessageSent => "message_sent".to_string(),
            ExpectedState::HelpVisible(b) => format!("help_visible == {b}"),
            ExpectedState::Generic(s) => s.clone(),
        }
    }
}

/// The lifecycle state of a single GUI action, from initiation to completion.
///
/// # State machine
///
/// ```text
/// Queued ──→ Validating ──→ Rejected (terminal)
///                 │
///                 └──→ AppMessageQueued ──→ AppMessageHandled ──→ Completed (terminal)
///                                                      │
///                                                      ├──→ Failed (terminal)
///                                                      │
///                                                      └──→ WaitingForExpectedState ──→ Completed (terminal)
///                                                                               │
///                                                                               └──→ TimedOut (terminal)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiActionState {
    /// Action has been queued but not yet processed.
    Queued,
    /// Action is being validated against current application state.
    Validating,
    /// Action validation failed; will not proceed.
    Rejected,
    /// Action could not be accepted because the bounded action queue was full.
    QueueFull,
    /// Action has been converted to an AppMessage and queued for processing.
    AppMessageQueued,
    /// AppMessage was handled by the application state layer.
    AppMessageHandled,
    /// Waiting for the UI to reflect the expected state change.
    WaitingForExpectedState,
    /// Action completed successfully (terminal).
    Completed,
    /// Action timed out waiting for completion (terminal).
    TimedOut,
    /// Action failed irrecoverably (terminal).
    Failed,
}

impl GuiActionState {
    /// Returns `true` if this is a terminal state (action is done or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            GuiActionState::Completed
                | GuiActionState::TimedOut
                | GuiActionState::Failed
                | GuiActionState::Rejected
                | GuiActionState::QueueFull
        )
    }

    /// Returns `true` if this is an active (non-terminal) state.
    pub fn is_active(&self) -> bool {
        !self.is_terminal()
    }
}

/// An incoming GUI action with structured metadata.
///
/// This is recorded when the user initiates an action through the GUI
/// (e.g. pressing Send, opening a room, toggling dark mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionRequest {
    /// Unique action identifier.
    pub action_id: GuiActionId,
    /// Unix epoch millisecond when the action was initiated in the GUI.
    pub requested_at_ms: i64,
    /// The command/action name (e.g. `"SendPressed"`, `"OpenRoom"`, `"AddFriend"`).
    pub command: String,
}

/// Current status of a GUI action, tracking its lifecycle through the system.
///
/// The status is updated as the action progresses through validation,
/// application message handling, and UI state observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionStatus {
    /// Unique action identifier.
    pub action_id: GuiActionId,
    /// Current lifecycle state.
    pub state: GuiActionState,
    /// Unix epoch millisecond when the action was first requested.
    pub requested_at_ms: i64,
    /// Unix epoch millisecond when the status was last updated.
    pub updated_at_ms: i64,
    /// The expected GUI revision number the action will produce, if known.
    pub expected_gui_revision: Option<u64>,
    /// The observed GUI revision number after the action was handled.
    pub observed_gui_revision: Option<u64>,
    /// Structured error if the action failed or was rejected.
    pub error: Option<GuiActionError>,
    /// Optional result payload (e.g. success message, created resource ID).
    pub result: Option<String>,
    /// The expected UI state condition that this action is waiting for,
    /// if any.  Set before the action enters `WaitingForExpectedState`
    /// and checked after the action is handled.
    pub expected_state: Option<ExpectedState>,
    /// Absolute timestamp (milliseconds since epoch) when this action
    /// should time out if still in `WaitingForExpectedState`.
    /// Set automatically when transitioning into that state.
    pub timeout_at_ms: Option<i64>,
}

impl GuiActionStatus {
    /// Transition the action to a new state, updating the timestamp.
    ///
    /// This is a raw transition; use [`GuiActionStatus::transition_to`] for
    /// validated state-machine transitions.
    pub fn set_state(&mut self, new_state: GuiActionState) {
        // Check before move — new_state is moved into self.state below
        let needs_timeout = new_state == GuiActionState::WaitingForExpectedState;

        self.state = new_state;
        self.updated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Automatically arm the timeout when entering WaitingForExpectedState
        if needs_timeout {
            self.timeout_at_ms = Some(
                self.updated_at_ms
                    .checked_add(DEFAULT_ACTION_STATE_TIMEOUT_MS)
                    .unwrap_or(self.updated_at_ms),
            );
        } else {
            // Clear timeout when leaving WaitingForExpectedState
            self.timeout_at_ms = None;
        }
    }

    /// Set the expected UI state condition for this action.
    ///
    /// Returns `&mut Self` for chaining.
    pub fn with_expected_state(&mut self, expected: ExpectedState) -> &mut Self {
        self.expected_state = Some(expected);
        self
    }

    /// Returns `true` if this action has an expected state that is
    /// matched by the given (category, value) observation.
    pub fn expected_state_matches(&self, category: &str, value: &str) -> bool {
        self.expected_state
            .as_ref()
            .map(|es| es.matches_str(category, value))
            .unwrap_or(false)
    }

    /// Attempt a validated state-machine transition.
    ///
    /// Returns `Ok(())` if the transition is valid, or `Err(GuiActionError)` with
    /// a structured error.
    ///
    /// Valid transitions:
    ///   `Queued`                    → `Validating` | `QueueFull`
    ///   `Validating`                → `Rejected` | `AppMessageQueued`
    ///   `AppMessageQueued`          → `AppMessageHandled`
    ///   `AppMessageHandled`         → `Completed` | `Failed` | `WaitingForExpectedState`
    ///   `WaitingForExpectedState`   → `Completed` | `TimedOut` | `Failed`
    ///   Terminal states             → (no transitions allowed)
    pub fn transition_to(&mut self, target: GuiActionState) -> Result<(), GuiActionError> {
        use GuiActionState::*;

        let allowed = matches!(
            (&self.state, &target),
            (Queued, Validating)
                | (Queued, QueueFull)
                | (Validating, Rejected)
                | (Validating, AppMessageQueued)
                | (AppMessageQueued, AppMessageHandled)
                | (AppMessageHandled, Completed)
                | (AppMessageHandled, Failed)
                | (AppMessageHandled, WaitingForExpectedState)
                | (WaitingForExpectedState, Completed)
                | (WaitingForExpectedState, TimedOut)
                | (WaitingForExpectedState, Failed)
        );

        if allowed {
            self.set_state(target);
            Ok(())
        } else {
            Err(GuiActionError::new(
                GuiActionErrorCode::InvalidArgument,
                format!("Invalid state transition: {:?} → {:?}", self.state, target),
            ))
        }
    }
}

/// Bounded, thread-safe history store for GUI action lifecycle tracking.
///
/// Stores up to `max_actions` entries.  Oldest **completed** (terminal)
/// actions are evicted first when the store is at capacity and a new
/// action is recorded.  A terminal action is one with a state of
/// [`GuiActionState::Completed`], [`GuiActionState::TimedOut`],
/// [`GuiActionState::Failed`], or [`GuiActionState::Rejected`].
/// Active (non-terminal) actions are never evicted automatically so
/// in-flight operations are never lost.
///
/// # Default capacity
///
/// | Store         | Max entries |
/// |---------------|-------------|
/// | Action history | 1 000       |
#[derive(Debug, Clone)]
pub struct GuiActionHistory {
    pub(crate) inner: Arc<GuiActionHistoryInner>,
}

#[derive(Debug)]
pub(crate) struct GuiActionHistoryInner {
    /// Map from action ID to status entry.
    pub(crate) actions: Mutex<HashMap<GuiActionId, GuiActionStatus>>,
    /// Insertion-order queue (action IDs, oldest first).  Used for eviction.
    order: Mutex<VecDeque<GuiActionId>>,
    /// Maximum number of stored actions.
    max_actions: usize,
}

impl GuiActionHistory {
    /// Create a new action history with the default capacity (1 000 actions).
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    /// Create a new action history with the given maximum number of actions.
    pub fn with_capacity(max_actions: usize) -> Self {
        let capped = max_actions.max(1).clamp(1, 5000);
        Self {
            inner: Arc::new(GuiActionHistoryInner {
                actions: Mutex::new(HashMap::with_capacity(capped + 32)),
                order: Mutex::new(VecDeque::with_capacity(capped + 32)),
                max_actions: capped,
            }),
        }
    }

    /// Record a new GUI action.
    ///
    /// If the store is at capacity, oldest terminal (completed) actions are
    /// evicted to make room.  Returns the action ID.
    pub fn record(&self, request: GuiActionRequest) -> GuiActionId {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let status = GuiActionStatus {
            action_id: request.action_id.clone(),
            state: GuiActionState::Queued,
            requested_at_ms: request.requested_at_ms,
            updated_at_ms: now_ms,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        let id = status.action_id.clone();

        let mut actions = self.inner.actions.lock().expect("actions lock");
        let mut order = self.inner.order.lock().expect("order lock");

        // MCP records an action at enqueue time and the Iced loop records the
        // same request when it receives it. Treat action_id as an idempotency
        // key so the second observation does not create a duplicate entry.
        if actions.contains_key(&request.action_id) {
            return request.action_id;
        }

        // Evict oldest terminal actions first. If none exist, evict the
        // oldest action (even active) to enforce the capacity bound.
        while actions.len() >= self.inner.max_actions {
            // Find the oldest terminal action from the front
            let terminal_pos = order.iter().position(|id| {
                actions
                    .get(id)
                    .map(|s| s.state.is_terminal())
                    .unwrap_or(false)
            });

            if let Some(pos) = terminal_pos {
                // Evict the first terminal action found
                if let Some(id) = order.remove(pos) {
                    actions.remove(&id);
                    continue;
                }
            }

            // No terminal action found — evict the oldest action (front)
            if let Some(oldest_id) = order.pop_front() {
                actions.remove(&oldest_id);
                continue;
            }

            // Order is empty — can't evict further
            break;
        }

        actions.insert(id.clone(), status);
        order.push_back(id.clone());

        id
    }

    /// Update the state of an existing action using validated state-machine
    /// transitions.  Returns `Ok(())` on success or `Err(GuiActionError)`
    /// with a structured error code for programmatic handling.
    pub fn transition_to(
        &self,
        action_id: &GuiActionId,
        target: GuiActionState,
    ) -> Result<(), GuiActionError> {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.transition_to(target)
        } else {
            Err(GuiActionError::new(
                GuiActionErrorCode::InvalidArgument,
                format!("Action {action_id} not found"),
            ))
        }
    }

    /// Update the state of an existing action directly (no validation).
    ///
    /// Returns `true` if the action was found and updated.
    pub fn set_state(&self, action_id: &GuiActionId, state: GuiActionState) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.set_state(state);
            true
        } else {
            false
        }
    }

    /// Expire an action that has not reached a terminal state.
    ///
    /// This is an explicit, event-driven operation: callers can schedule one
    /// timer per action and invoke this method when it fires. Completed,
    /// rejected, and failed actions are never changed, so a late timer cannot
    /// interfere with unrelated GUI work.
    pub fn expire(&self, action_id: &GuiActionId) -> Option<GuiActionStatus> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let mut actions = self.inner.actions.lock().expect("actions lock");
        let status = actions.get_mut(action_id)?;
        if status.state.is_terminal() {
            return None;
        }
        status.state = GuiActionState::TimedOut;
        status.updated_at_ms = now_ms;
        status.timeout_at_ms = None;
        status.error = Some(GuiActionError::new(
            GuiActionErrorCode::ActionTimedOut,
            format!("GUI action timed out after {DEFAULT_ACTION_STATE_TIMEOUT_MS}ms"),
        ));
        Some(status.clone())
    }

    /// Set the error details on an existing action.
    ///
    /// Returns `true` if the action was found and updated, `false` otherwise.
    pub fn set_error(&self, action_id: &GuiActionId, error: GuiActionError) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.error = Some(error);
            true
        } else {
            false
        }
    }

    /// Retrieve the status of an action by its ID.
    pub fn get(&self, action_id: &GuiActionId) -> Option<GuiActionStatus> {
        let actions = self.inner.actions.lock().expect("actions lock");
        actions.get(action_id).cloned()
    }

    /// Return all stored actions, newest first.
    pub fn all_actions(&self) -> Vec<GuiActionStatus> {
        let actions = self.inner.actions.lock().expect("actions lock");
        let order = self.inner.order.lock().expect("order lock");
        order
            .iter()
            .rev()
            .filter_map(|id| actions.get(id))
            .cloned()
            .collect()
    }

    /// Return actions matching a specific state, newest first.
    pub fn actions_with_state(&self, state: GuiActionState) -> Vec<GuiActionStatus> {
        self.all_actions()
            .into_iter()
            .filter(|a| a.state == state)
            .collect()
    }

    /// Return the total number of stored actions.
    pub fn action_count(&self) -> usize {
        let actions = self.inner.actions.lock().expect("actions lock");
        actions.len()
    }

    /// Return the number of active (non-terminal) actions.
    pub fn active_count(&self) -> usize {
        let actions = self.inner.actions.lock().expect("actions lock");
        actions.values().filter(|a| a.state.is_active()).count()
    }

    /// Set the expected completion state for a recorded action.
    ///
    /// Returns `true` if the action was found and updated.
    pub fn set_expected_state(&self, action_id: &GuiActionId, expected: ExpectedState) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(action_id) {
            status.expected_state = Some(expected);
            true
        } else {
            false
        }
    }

    /// Remove an action by ID.  Returns `true` if it existed.
    pub fn remove(&self, action_id: &GuiActionId) -> bool {
        let mut actions = self.inner.actions.lock().expect("actions lock");
        let mut order = self.inner.order.lock().expect("order lock");

        let existed = actions.remove(action_id).is_some();
        if existed {
            // Remove from order list
            if let Some(pos) = order.iter().position(|id| id == action_id) {
                order.remove(pos);
            }
        }
        existed
    }

    /// Check for actions whose timeout has expired and transition them
    /// to `TimedOut`.
    ///
    /// Returns a list of `(action_id, status)` pairs for each action that
    /// was transitioned to `TimedOut`.  Only actions in
    /// `WaitingForExpectedState` with elapsed `timeout_at_ms` are affected.
    ///
    /// This is the main timeout enforcement mechanism.  Call it before
    /// querying action status (via `get`, `all_actions`, etc.) to ensure
    /// expired actions are detected.
    ///
    /// # No busy polling
    ///
    /// To avoid polling, use [`GuiActionHistory::next_timeout_remaining_ms`]
    /// to find out when the next timeout will expire, then schedule a single
    /// timer for that moment.
    pub fn check_timeouts(&self) -> Vec<(GuiActionId, GuiActionStatus)> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let mut actions = self.inner.actions.lock().expect("actions lock");

        let mut timed_out: Vec<(GuiActionId, GuiActionStatus)> = Vec::new();

        // Collect IDs of expired actions first (to avoid borrow conflicts)
        let expired_ids: Vec<GuiActionId> = actions
            .iter()
            .filter(|(_, status)| {
                status.state == GuiActionState::WaitingForExpectedState
                    && status.timeout_at_ms.map(|t| now_ms >= t).unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in &expired_ids {
            if let Some(status) = actions.get_mut(id) {
                status.state = GuiActionState::TimedOut;
                status.updated_at_ms = now_ms;
                status.timeout_at_ms = None;
                timed_out.push((id.clone(), status.clone()));
            }
        }

        timed_out
    }

    /// Returns the remaining milliseconds until the next action timeout
    /// expires, or `None` if no action is currently timing.
    ///
    /// Use this to schedule a single timer instead of polling:
    ///
    /// ```ignore
    /// if let Some(remaining_ms) = history.next_timeout_remaining_ms() {
    ///     tokio::time::sleep(Duration::from_millis(remaining_ms)).await;
    ///     history.check_timeouts();
    /// }
    /// ```
    pub fn next_timeout_remaining_ms(&self) -> Option<u64> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let actions = self.inner.actions.lock().expect("actions lock");

        let earliest = actions
            .values()
            .filter(|s| {
                s.state == GuiActionState::WaitingForExpectedState && s.timeout_at_ms.is_some()
            })
            .filter_map(|s| s.timeout_at_ms)
            .min()?;

        let remaining = earliest - now_ms;
        if remaining <= 0 {
            Some(0)
        } else {
            Some(remaining as u64)
        }
    }
}

impl Default for GuiActionHistory {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// GUI Wait Conditions
// =============================================================================

/// Serializable GUI wait conditions for diagnostic polling.
///
/// Each variant describes a condition that can be evaluated against
/// the current [`IcedStateSnapshot`] and [`IcedMessageJournal`].
///
/// Only variants supported by current diagnostics data (as exposed
/// by these two types) are included.
///
/// # Evaluation
///
/// Use [`evaluate_wait_condition`] to check whether a condition is
/// currently satisfied.
///
/// # Security
///
/// - No secrets (keys, tickets, tokens) are exposed.
/// - String parameters are bounded at 4096 characters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuiWaitCondition {
    /// Whether the active screen name matches the expected value.
    ///
    /// Evaluated against [`IcedStateSnapshot::active_screen`].
    /// Example expected values: `"ChatList"`, `"Chat"`, `"Settings"`.
    ScreenIs {
        /// Expected screen name.
        expected: String,
    },
    /// Whether a room is currently selected (open), optionally matching
    /// a specific room topic.
    ///
    /// Evaluated against [`IcedStateSnapshot::active_room`].
    RoomSelected {
        /// If `Some(topic)`, requires the active room to match this topic.
        /// If `None`, any active room is sufficient.
        room_topic: Option<String>,
    },
    /// Whether at least `min_count` gossip peers are visible as neighbors.
    ///
    /// Evaluated against [`IcedStateSnapshot::neighbor_count`].
    PeerVisible {
        /// Minimum number of peers that must be visible.
        min_count: u32,
    },
    /// Whether at least `min_count` chat entries (messages) are present
    /// in the active conversation.
    ///
    /// Evaluated against [`IcedStateSnapshot::total_entry_count`].
    MessageVisible {
        /// Minimum number of chat entries that must be visible.
        min_count: u32,
    },
    /// Whether the GUI revision counter (the Iced message journal's latest
    /// sequence number) has reached at least `expected_revision`.
    ///
    /// Evaluated against [`IcedMessageJournal::latest_sequence`].
    /// This is a monotonic counter that increments each time an Iced
    /// `AppMessage` is processed — useful for waiting until pending
    /// state updates have been handled.
    GuiRevisionAtLeast {
        /// The minimum revision number that must have been reached.
        expected_revision: u64,
    },
    /// Whether a conversation (room or direct) is currently open, optionally
    /// matching a specific conversation identifier.
    ///
    /// Evaluated against [`IcedStateSnapshot::active_room`].
    /// This is a more general check than [`RoomSelected`](crate::diagnostics::GuiWaitCondition::RoomSelected) — it covers both
    /// group chat rooms and direct message conversations.
    ConversationSelected {
        /// If `Some(id)`, requires the active conversation to match this
        /// room topic or peer public key (hex). If `None`, any active
        /// conversation is sufficient.
        conversation_id: Option<String>,
    },
    /// Whether the composer text for the active conversation matches the
    /// expected value exactly.
    ///
    /// Evaluated against [`IcedStateSnapshot::composer_text`].
    ComposerTextIs {
        /// Expected composer text content.
        expected: String,
    },
    /// Whether dark mode has the requested value.
    DarkModeIs {
        /// Expected dark-mode setting.
        expected: bool,
    },
    /// Whether a previously submitted GUI action has reached a state.
    ActionStatusIs {
        /// Action id to inspect in [`GuiActionHistory`].
        action_id: String,
        /// Required lifecycle state.
        expected: GuiActionState,
    },
    /// Whether a blocking modal dialog is currently open.
    ///
    /// Evaluated against [`IcedStateSnapshot::dialog_open`].
    /// Common examples include the help overlay, confirmation dialogs, and
    /// error modals.
    DialogOpen,
    /// Whether no blocking modal dialog is currently open.
    ///
    /// Evaluated against [`IcedStateSnapshot::dialog_open`].
    /// The logical inverse of [`DialogOpen`](crate::diagnostics::GuiWaitCondition::DialogOpen).
    DialogClosed,
    /// Whether the total number of unread messages across all conversations
    /// is at least `min_count`.
    ///
    /// Evaluated against [`IcedStateSnapshot::unread_count`].
    UnreadCountAtLeast {
        /// Minimum number of unread messages that must be pending.
        min_count: u32,
    },
}

/// Evaluate a [`GuiWaitCondition`] against the current diagnostics state.
///
/// Returns `true` if the condition is satisfied, `false` otherwise.
///
/// # Examples
///
/// ```ignore
/// use crate::diagnostics::{GuiWaitCondition, IcedStateSnapshot, IcedMessageJournal, evaluate_wait_condition};
///
/// let snapshot = IcedStateSnapshot { /* ... */ };
/// let journal = IcedMessageJournal::new();
///
/// let condition = GuiWaitCondition::ScreenIs {
///     expected: "ChatList".to_string(),
/// };
///
/// if evaluate_wait_condition(&condition, &snapshot, &journal) {
///     // Screen is ChatList
/// }
/// ```
pub fn evaluate_wait_condition(
    condition: &GuiWaitCondition,
    snapshot: &IcedStateSnapshot,
    journal: &IcedMessageJournal,
) -> bool {
    match condition {
        GuiWaitCondition::ScreenIs { expected } => snapshot.active_screen == *expected,
        GuiWaitCondition::RoomSelected { room_topic } => match room_topic {
            Some(topic) => snapshot.active_room.as_deref() == Some(topic.as_str()),
            None => snapshot.active_room.is_some(),
        },
        GuiWaitCondition::PeerVisible { min_count } => {
            snapshot.neighbor_count >= *min_count as usize
        }
        GuiWaitCondition::MessageVisible { min_count } => {
            snapshot.total_entry_count >= *min_count as usize
        }
        GuiWaitCondition::GuiRevisionAtLeast { expected_revision } => {
            journal.latest_sequence() >= *expected_revision
        }
        GuiWaitCondition::ConversationSelected { conversation_id } => match conversation_id {
            Some(id) => snapshot.active_room.as_deref() == Some(id.as_str()),
            None => snapshot.active_room.is_some(),
        },
        GuiWaitCondition::ComposerTextIs { expected } => snapshot.composer_text == *expected,
        GuiWaitCondition::DarkModeIs { expected } => snapshot.dark_mode == *expected,
        // Action predicates require the history-aware evaluator below.  The
        // snapshot-only API must never report them as observed.
        GuiWaitCondition::ActionStatusIs { .. } => false,
        GuiWaitCondition::DialogOpen => snapshot.dialog_open,
        GuiWaitCondition::DialogClosed => !snapshot.dialog_open,
        GuiWaitCondition::UnreadCountAtLeast { min_count } => {
            snapshot.unread_count >= *min_count as usize
        }
    }
}

/// Evaluate a GUI wait condition, including action lifecycle predicates.
///
/// Action conditions are true only when the requested action exists and its
/// observed state exactly equals the expected state.  This prevents a wait
/// from succeeding on an unobserved or merely queued action.
pub fn evaluate_wait_condition_with_actions(
    condition: &GuiWaitCondition,
    snapshot: &IcedStateSnapshot,
    journal: &IcedMessageJournal,
    actions: &GuiActionHistory,
) -> bool {
    if let GuiWaitCondition::ActionStatusIs {
        action_id,
        expected,
    } = condition
    {
        return actions
            .get(&GuiActionId(action_id.clone()))
            .is_some_and(|status| status.state == *expected);
    }
    evaluate_wait_condition(condition, snapshot, journal)
}

// =============================================================================
// GUI Test Command Types
// =============================================================================

/// Maximum length of any user-facing string parameter in GUI test commands.
pub const GUI_TEST_COMMAND_MAX_STRING_LEN: usize = 4096;

/// Maximum timeout for wait conditions (milliseconds).
pub const GUI_TEST_COMMAND_MAX_TIMEOUT_MS: u64 = 30_000;

/// Default timeout for an action to reach the expected state (milliseconds).
/// Used when no explicit timeout is specified; 10 seconds.
pub const DEFAULT_ACTION_STATE_TIMEOUT_MS: i64 = 10_000;

/// Maximum permitted timeout for an action to reach the expected state (milliseconds).
/// Hard upper bound — 30 seconds.
pub const MAX_ACTION_STATE_TIMEOUT_MS: i64 = 30_000;

/// High-level GUI test commands that an AI agent can issue.
///
/// Each variant describes a semantic GUI action — no pixel coordinates,
/// no keyboard injection, no shell commands, no file system paths.
///
/// Only commands that map to existing GUI behaviour in the Iced chat
/// frontend are included.  All identifiers are hex-encoded strings.
///
/// # Security
///
/// - No secrets (keys, tickets, tokens) are exposed.
/// - String parameters are bounded at [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars.
/// - No arbitrary widget IDs, coordinates, or shell commands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardTabName {
    /// The "Shared by Me" tab of the File Sharing dashboard (files this
    /// node registers for sharing).
    FilesSharing,
    /// The "Downloading" tab (in-progress downloads from peers).
    Downloading,
    /// The "Downloaded" tab (completed downloads).
    Downloaded,
    /// The "Shared with Me" tab (files shared to this node).
    SharedWithMe,
    /// The "Activity Log" tab (transfer lifecycle events).
    Activity,
}

impl DashboardTabName {
    /// Return the JSON string representation of this tab.
    pub fn as_str(&self) -> &'static str {
        match self {
            DashboardTabName::FilesSharing => "files_sharing",
            DashboardTabName::Downloading => "downloading",
            DashboardTabName::Downloaded => "downloaded",
            DashboardTabName::SharedWithMe => "shared_with_me",
            DashboardTabName::Activity => "activity",
        }
    }

    /// Parse a tab name string into a [`DashboardTabName`].
    ///
    /// Returns `None` if the string is not one of the allowed values.
    pub fn from_str(s: &str) -> Option<DashboardTabName> {
        match s {
            "files_sharing" => Some(DashboardTabName::FilesSharing),
            "downloading" => Some(DashboardTabName::Downloading),
            "downloaded" => Some(DashboardTabName::Downloaded),
            "shared_with_me" => Some(DashboardTabName::SharedWithMe),
            "activity" => Some(DashboardTabName::Activity),
            _ => None,
        }
    }

    /// Iterate over all supported tab name strings.
    pub fn all_names() -> &'static [&'static str] {
        &[
            "files_sharing",
            "downloading",
            "downloaded",
            "shared_with_me",
            "activity",
        ]
    }
}

impl std::fmt::Display for DashboardTabName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
/// Commands that drive the GUI through test harness or MCP integration.
///
/// Each variant maps to a specific user-facing action (navigation,
/// screen selection, composer control, etc.) and is gated behind
/// the `--enable-gui-test-actions` flag for safety.
pub enum GuiTestCommand {
    /// Navigate to the chat list (home) screen.
    GoToChatList,
    /// Open a specific room by its topic ID.
    OpenRoom {
        /// Room topic ID as a hex string.
        room_id: String,
    },
    /// Open a direct conversation with a peer.
    OpenConversation {
        /// Peer public key as a hex string.
        conversation_id: String,
    },
    /// Open the friend requests screen.
    OpenFriends,
    /// Open the settings screen.
    OpenSettings,
    /// Open the file sharing dashboard screen.
    OpenFileSharing,
    /// Close the currently open dialog or settings screen.
    CloseDialog,
    /// Set the composer (message input) text for the active conversation.
    SetComposerText {
        /// Text to insert into the composer (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars,
        /// no control characters).
        text: String,
    },
    /// Submit the composer — sends whatever is currently in the composer
    /// for the active conversation.
    SubmitComposer,
    /// Clear the composer through the normal input-change state path.
    ClearComposer,
    /// Focus the composer input through the native GUI focus operation.
    FocusComposer,
    /// Select a peer by public key to view their profile or open a conversation.
    SelectPeer {
        /// Peer public key as a hex string.
        peer_id: String,
    },
    /// Toggle dark mode on/off.
    ToggleDarkMode {
        /// Target state: `true` = dark, `false` = light.
        enabled: bool,
    },
    /// Toggle the help overlay.
    ToggleHelp,
    /// Browse a remote peer's shared file catalogue.
    BrowseCatalogue {
        /// Peer public key as a hex string.
        peer_id: String,
    },
    /// Download a file from a remote peer's catalogue.
    DownloadFile {
        /// Peer public key as a hex string.
        peer_id: String,
        /// Content hash (blake3) of the file to download.
        content_hash: String,
    },
    /// Simulate a peer's online/offline presence for GUI testing.
    ///
    /// The simulated event is routed through the same friend-status path used
    /// by real network events, so the conversation header derives its presence
    /// label and dot from the production `peer_presence_map` code path. This
    /// lets screenshot harnesses exercise the Online/Offline header states
    /// without requiring a live peer connection.
    SetPeerPresence {
        /// Peer public key as a hex string.
        peer_id: String,
        /// `true` = online (fresh last-seen timestamp), `false` = offline.
        online: bool,
    },
    /// Clear the in-memory mesh event log shown on the home Mesh Health card.
    ///
    /// Test-only: lets screenshot harnesses capture the intentional no-events
    /// state of the card without waiting for a real network lifecycle. It
    /// never fabricates events — it only removes the real log lines.
    ClearMeshEventLog,
    /// Open the create-room dialog.
    CreateNewRoom,
    /// Set the room name in the create-room dialog.
    SetCreateRoomName {
        /// Room name text (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars).
        name: String,
    },
    /// Toggle the "Advertise in Directory" checkbox in the create-room dialog.
    ///
    /// Backward-compatible alias for [`GuiTestCommand::SetCreateRoomVisibility`]:
    /// `true` → [`RoomVisibility::PublicDiscoverable`], `false` →
    /// [`RoomVisibility::Private`] (the pre-BORU-DIR-05 behaviour where an
    /// unchecked checkbox created a private room).
    SetCreateRoomAdvertise {
        /// `true` to check (discoverable public), `false` to uncheck (private).
        enabled: bool,
    },
    /// Set the room visibility in the create-room dialog (BORU-DIR-05).
    SetCreateRoomVisibility {
        /// The visibility to select.
        visibility: RoomVisibility,
    },
    /// Set the optional description in the create-room dialog (BORU-DIR-05).
    SetCreateRoomDescription {
        /// Description text (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars).
        description: String,
    },
    /// Set the optional comma-separated tags in the create-room dialog (BORU-DIR-05).
    SetCreateRoomTags {
        /// Comma-separated tag text (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars).
        tags: String,
    },
    /// Open the room-settings dialog for an existing room (BORU-DIR-06).
    OpenRoomSettings {
        /// Room topic ID as a hex string.
        room_id: String,
    },
    /// Set the room name in the room-settings dialog (BORU-DIR-06).
    SetRoomSettingsName {
        /// Room name text (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars).
        name: String,
    },
    /// Set the description in the room-settings dialog (BORU-DIR-06).
    SetRoomSettingsDescription {
        /// Description text (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars).
        description: String,
    },
    /// Set the comma-separated tags in the room-settings dialog (BORU-DIR-06).
    SetRoomSettingsTags {
        /// Comma-separated tag text (max [`GUI_TEST_COMMAND_MAX_STRING_LEN`] chars).
        tags: String,
    },
    /// Set the visibility in the room-settings dialog (BORU-DIR-06).
    SetRoomSettingsVisibility {
        /// The visibility to select.
        visibility: RoomVisibility,
    },
    /// Apply the room-settings dialog (BORU-DIR-06).
    ConfirmRoomSettings,
    /// Direct owner/admin switch of an existing room's directory visibility
    /// (BORU-DIR-06, PDF Task 2.3): PublicDiscoverable <-> PublicUnlisted.
    SetRoomDirectoryVisibility {
        /// Room topic ID as a hex string.
        room_id: String,
        /// The visibility to switch to.
        visibility: RoomVisibility,
    },
    /// Confirm and create the room from the dialog's current settings.
    ConfirmCreateNewRoom,
    /// Switch the File Sharing dashboard to a specific tab.
    ///
    /// The File Sharing screen is opened first if it is not already active.
    /// This lets test harnesses inspect a specific dashboard tab without
    /// clicking through the sidebar.
    OpenDashboardTab {
        /// Which dashboard tab to activate.
        tab: DashboardTabName,
    },
    /// Register a local file for sharing by absolute path.
    ///
    /// Test-only shortcut that bypasses the native OS file picker
    /// (rfd / xdg-desktop-portal) and routes directly through the same
    /// [`crate::diagnostics::GuiTestCommand`] pipeline as the production
    /// share flow.  The path must exist and be readable.
    TestShareFile {
        /// Absolute path to the file to register for sharing.
        path: String,
    },
    /// Wait for a GUI condition to be satisfied.
    Wait {
        /// The condition to evaluate.
        condition: GuiWaitCondition,
        /// Maximum wait time in milliseconds (max [`GUI_TEST_COMMAND_MAX_TIMEOUT_MS`]).
        timeout_ms: u64,
    },
}

/// Validate an identifier supplied to a GUI action.  GUI actions are semantic
/// commands, not a general-purpose string or command interpreter, so IDs are
/// deliberately restricted to the portable identifier alphabet.
fn validate_gui_identifier(value: &str, name: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    let length = value.chars().count();
    if length > GUI_TEST_COMMAND_MAX_STRING_LEN {
        return Err(format!(
            "{name} too long ({length} chars, max {})",
            GUI_TEST_COMMAND_MAX_STRING_LEN
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(format!(
            "{name} contains invalid characters; only ASCII letters, digits, '-' and '_' are allowed"
        ));
    }
    Ok(())
}

fn validate_gui_text(value: &str, name: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length > GUI_TEST_COMMAND_MAX_STRING_LEN {
        return Err(format!(
            "{name} too long ({length} chars, max {})",
            GUI_TEST_COMMAND_MAX_STRING_LEN
        ));
    }
    if value.chars().any(|c| c.is_control() && c != ' ') {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(())
}

impl GuiTestCommand {
    /// Validate the command parameters.
    ///
    /// Returns `Ok(())` if the command is well-formed, or an error message.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            GuiTestCommand::OpenRoom { room_id } => validate_gui_identifier(room_id, "room_id"),
            GuiTestCommand::OpenConversation { conversation_id } => {
                validate_gui_identifier(conversation_id, "conversation_id")
            }
            GuiTestCommand::SetComposerText { text } => validate_gui_text(text, "Composer text"),
            GuiTestCommand::SelectPeer { peer_id } => validate_gui_identifier(peer_id, "peer_id"),
            GuiTestCommand::ToggleDarkMode { .. } => Ok(()),
            GuiTestCommand::ToggleHelp => Ok(()),
            GuiTestCommand::GoToChatList => Ok(()),
            GuiTestCommand::OpenFriends => Ok(()),
            GuiTestCommand::SubmitComposer => Ok(()),
            GuiTestCommand::ClearComposer => Ok(()),
            GuiTestCommand::FocusComposer => Ok(()),
            GuiTestCommand::OpenSettings => Ok(()),
            GuiTestCommand::OpenFileSharing => Ok(()),
            GuiTestCommand::CloseDialog => Ok(()),
            GuiTestCommand::CreateNewRoom => Ok(()),
            GuiTestCommand::SetCreateRoomName { name } => validate_gui_text(name, "Room name"),
            GuiTestCommand::SetCreateRoomAdvertise { .. } => Ok(()),
            GuiTestCommand::SetCreateRoomVisibility { .. } => Ok(()),
            GuiTestCommand::SetCreateRoomDescription { description } => {
                validate_gui_text(description, "Room description")
            }
            GuiTestCommand::SetCreateRoomTags { tags } => validate_gui_text(tags, "Room tags"),
            GuiTestCommand::OpenRoomSettings { room_id } => {
                validate_gui_identifier(room_id, "room_id")
            }
            GuiTestCommand::SetRoomSettingsName { name } => validate_gui_text(name, "Room name"),
            GuiTestCommand::SetRoomSettingsDescription { description } => {
                validate_gui_text(description, "Room description")
            }
            GuiTestCommand::SetRoomSettingsTags { tags } => validate_gui_text(tags, "Room tags"),
            GuiTestCommand::SetRoomSettingsVisibility { .. } => Ok(()),
            GuiTestCommand::ConfirmRoomSettings => Ok(()),
            GuiTestCommand::SetRoomDirectoryVisibility { room_id, .. } => {
                validate_gui_identifier(room_id, "room_id")
            }
            GuiTestCommand::ConfirmCreateNewRoom => Ok(()),
            GuiTestCommand::BrowseCatalogue { peer_id } => {
                validate_gui_identifier(peer_id, "peer_id")
            }
            GuiTestCommand::DownloadFile {
                peer_id,
                content_hash,
            } => {
                validate_gui_identifier(peer_id, "peer_id")?;
                validate_gui_identifier(content_hash, "content_hash")
            }
            GuiTestCommand::SetPeerPresence { peer_id, .. } => {
                validate_gui_identifier(peer_id, "peer_id")
            }
            GuiTestCommand::ClearMeshEventLog => Ok(()),
            GuiTestCommand::OpenDashboardTab { tab } => {
                // The tab is a closed serde enum — every value is valid.
                let _ = tab;
                Ok(())
            }
            GuiTestCommand::TestShareFile { path } => validate_gui_text(path, "path"),
            GuiTestCommand::Wait {
                condition,
                timeout_ms,
            } => {
                if *timeout_ms > GUI_TEST_COMMAND_MAX_TIMEOUT_MS {
                    return Err(format!(
                        "Timeout must not exceed {}ms",
                        GUI_TEST_COMMAND_MAX_TIMEOUT_MS
                    ));
                }
                match condition {
                    GuiWaitCondition::ScreenIs { expected } => {
                        validate_gui_identifier(expected, "expected screen name")?;
                    }
                    GuiWaitCondition::RoomSelected { room_topic } => {
                        if let Some(topic) = room_topic {
                            validate_gui_identifier(topic, "room topic")?;
                        }
                    }
                    GuiWaitCondition::PeerVisible { .. }
                    | GuiWaitCondition::MessageVisible { .. }
                    | GuiWaitCondition::GuiRevisionAtLeast { .. }
                    | GuiWaitCondition::DarkModeIs { .. }
                    | GuiWaitCondition::DialogOpen
                    | GuiWaitCondition::DialogClosed
                    | GuiWaitCondition::UnreadCountAtLeast { .. } => {}
                    GuiWaitCondition::ConversationSelected { conversation_id } => {
                        if let Some(id) = conversation_id {
                            validate_gui_identifier(id, "conversation_id")?;
                        }
                    }
                    GuiWaitCondition::ComposerTextIs { expected } => {
                        validate_gui_text(expected, "expected composer text")?;
                    }
                    GuiWaitCondition::ActionStatusIs { action_id, .. } => {
                        validate_gui_identifier(action_id, "action_id")?;
                    }
                }
                Ok(())
            }
        }
    }

    /// Return the expected state that the UI should be in after this
    /// command completes successfully, if one can be determined statically.
    ///
    /// Commands whose post-condition depends on current application state
    /// (e.g. `CloseDialog`, `SelectPeer`) return `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use boru_core::diagnostics::{GuiTestCommand, ExpectedState};
    ///
    /// let cmd = GuiTestCommand::GoToChatList;
    /// assert_eq!(cmd.expected_state(), Some(ExpectedState::ScreenIs("ChatList".into())));
    ///
    /// let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
    /// assert_eq!(cmd.expected_state(), Some(ExpectedState::DarkModeIs(true)));
    ///
    /// let cmd = GuiTestCommand::SubmitComposer;
    /// assert_eq!(cmd.expected_state(), Some(ExpectedState::MessageSent));
    ///
    /// let cmd = GuiTestCommand::CloseDialog;
    /// assert!(cmd.expected_state().is_none());
    /// ```
    pub fn expected_state(&self) -> Option<ExpectedState> {
        match self {
            GuiTestCommand::GoToChatList => Some(ExpectedState::ScreenIs("ChatList".into())),
            GuiTestCommand::OpenRoom { room_id } => {
                Some(ExpectedState::RoomSelected(room_id.clone()))
            }
            GuiTestCommand::OpenConversation { conversation_id } => {
                Some(ExpectedState::ConversationSelected(conversation_id.clone()))
            }
            GuiTestCommand::SetComposerText { text } => {
                Some(ExpectedState::ComposerTextIs(text.clone()))
            }
            GuiTestCommand::SubmitComposer => Some(ExpectedState::MessageSent),
            GuiTestCommand::ClearComposer => Some(ExpectedState::ComposerTextIs(String::new())),
            GuiTestCommand::FocusComposer => {
                Some(ExpectedState::Generic("composer_focused".into()))
            }
            GuiTestCommand::ToggleDarkMode { enabled } => Some(ExpectedState::DarkModeIs(*enabled)),
            GuiTestCommand::ToggleHelp => Some(ExpectedState::HelpVisible(true)),
            GuiTestCommand::OpenFriends => Some(ExpectedState::ScreenIs("FriendRequests".into())),
            GuiTestCommand::OpenSettings => Some(ExpectedState::ScreenIs("Settings".into())),
            GuiTestCommand::OpenFileSharing => Some(ExpectedState::ScreenIs("FileSharing".into())),
            // CloseDialog: depends on what screen was behind the dialog.
            GuiTestCommand::CloseDialog => None,
            // SelectPeer: may open a conversation or profile — depends on context.
            GuiTestCommand::SelectPeer { .. } => None,
            // Wait: the condition itself IS the post-condition, tracked separately.
            GuiTestCommand::Wait { .. } => None,
            // BrowseCatalogue: transitions to the peer's catalogue screen.
            GuiTestCommand::BrowseCatalogue { peer_id } => {
                Some(ExpectedState::ScreenIs(format!("PeerCatalogue({peer_id})")))
            }
            // DownloadFile: initiates a download — post-condition is tracked through
            // the DownloadInitiated event in the action lifecycle.
            GuiTestCommand::DownloadFile { .. } => {
                Some(ExpectedState::Generic("download_initiated".into()))
            }
            // SetPeerPresence: the resulting header state depends on the peer and
            // the current presence map — verified by screenshot, not statically.
            GuiTestCommand::SetPeerPresence { .. } => None,
            // ClearMeshEventLog: the resulting card state is verified by
            // screenshot, not statically.
            GuiTestCommand::ClearMeshEventLog => None,
            GuiTestCommand::CreateNewRoom => {
                Some(ExpectedState::Generic("create_room_dialog_open".into()))
            }
            GuiTestCommand::SetCreateRoomName { .. } => None,
            GuiTestCommand::SetCreateRoomAdvertise { .. } => None,
            GuiTestCommand::SetCreateRoomVisibility { .. } => None,
            GuiTestCommand::SetCreateRoomDescription { .. } => None,
            GuiTestCommand::SetCreateRoomTags { .. } => None,
            GuiTestCommand::OpenRoomSettings { .. } => {
                Some(ExpectedState::Generic("room_settings_dialog_open".into()))
            }
            GuiTestCommand::SetRoomSettingsName { .. } => None,
            GuiTestCommand::SetRoomSettingsDescription { .. } => None,
            GuiTestCommand::SetRoomSettingsTags { .. } => None,
            GuiTestCommand::SetRoomSettingsVisibility { .. } => None,
            GuiTestCommand::ConfirmRoomSettings => {
                Some(ExpectedState::Generic("room_settings_saved".into()))
            }
            GuiTestCommand::SetRoomDirectoryVisibility { .. } => Some(ExpectedState::Generic(
                "room_directory_visibility_switched".into(),
            )),
            GuiTestCommand::ConfirmCreateNewRoom => Some(ExpectedState::MessageSent),
            // OpenDashboardTab: opens the File Sharing screen and selects the
            // tab. The post-condition is tracked via the generic
            // "dashboard_tab_<name>" marker completed by the tab handler.
            GuiTestCommand::OpenDashboardTab { tab } => {
                Some(ExpectedState::Generic(format!("dashboard_tab_{tab}")))
            }
            // TestShareFile: depends on the file existing and being readable —
            // completed by the SharedFileAdded handler when registration
            // succeeds.
            GuiTestCommand::TestShareFile { .. } => None,
        }
    }
}

// =============================================================================
// GUI Action Event Journal (event-oriented journal, complement to state-based GuiActionHistory)
// =============================================================================

/// The kind of a GUI action diagnostic event.
///
/// These are high-level lifecycle events tracked through a bounded journal,
/// complementary to the state-machine tracking in [`GuiActionHistory`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuiActionEventKind {
    /// An action was initiated by the user or system.
    ActionRequested,
    /// An action was queued for processing.
    ActionQueued,
    /// Action validation has started.
    ActionValidationStarted,
    /// Action validation succeeded.
    ActionValidated,
    /// Action was rejected by validation.
    ActionRejected {
        /// Reason for rejection.
        reason: String,
    },
    /// Action could not be accepted because the bounded action queue was full.
    ActionQueueFull {
        /// Queue capacity at the time the action was rejected.
        capacity: usize,
    },
    /// An AppMessage was queued as a result of this action.
    AppMessageQueued {
        /// The AppMessage variant that was queued.
        message_variant: String,
    },
    /// An AppMessage was handled by the update handler.
    AppMessageHandled {
        /// The AppMessage variant that was handled.
        message_variant: String,
        /// Whether processing succeeded.
        success: bool,
    },
    /// The expected state was observed after an action was triggered.
    ExpectedStateObserved,
    /// An action completed successfully.
    ActionCompleted,
    /// An action timed out while waiting.
    ActionTimedOut {
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// An action failed with an error.
    ActionFailed {
        /// Error description.
        error: String,
    },
}

/// A single GUI action diagnostic event entry in the bounded journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionEvent {
    /// Monotonically increasing sequence number.
    pub sequence: u64,
    /// Wall-clock timestamp when the event was recorded.
    pub timestamp: DateTime<Utc>,
    /// Unique action identifier (maps to [`GuiActionId`]).
    pub action_id: String,
    /// The event kind and its payload.
    pub kind: GuiActionEventKind,
    /// GUI revision counter at the time the event was recorded.
    pub gui_revision: u64,
    /// Optional room/conversation identifier.
    pub room_id: Option<TopicId>,
    /// Current screen name (e.g. "ChatList", "Chat", "Settings").
    pub current_screen: String,
}

/// Thread-safe bounded journal of recent GUI action diagnostic events.
///
/// Records the last N [`GuiActionEvent`] values as they are emitted during
/// GUI action lifecycle tracking.  Oldest entries are automatically evicted
/// when the limit is exceeded.
///
/// # Defaults
///
/// | Store         | Max entries |
/// |---------------|-------------|
/// | Journal       | 1 000       |
#[derive(Debug, Clone)]
pub struct GuiActionEventHistory {
    inner: Arc<GuiActionEventHistoryInner>,
}

#[derive(Debug)]
struct GuiActionEventHistoryInner {
    entries: Mutex<VecDeque<GuiActionEvent>>,
    next_sequence: AtomicU64,
    max_entries: usize,
}

impl GuiActionEventHistory {
    /// Create a new action event journal with the default capacity (1 000 entries).
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    /// Create a new action event journal with the given maximum number of entries.
    pub fn with_capacity(max_entries: usize) -> Self {
        let capped = max_entries.clamp(64, 5000);
        Self {
            inner: Arc::new(GuiActionEventHistoryInner {
                entries: Mutex::new(VecDeque::with_capacity(capped + 32)),
                next_sequence: AtomicU64::new(0),
                max_entries: capped,
            }),
        }
    }

    /// Record a GUI action diagnostic event in the journal.
    pub fn record(
        &self,
        action_id: impl AsRef<str>,
        kind: GuiActionEventKind,
        gui_revision: u64,
        room_id: Option<TopicId>,
        current_screen: impl AsRef<str>,
    ) {
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        let entry = GuiActionEvent {
            sequence,
            timestamp: Utc::now(),
            action_id: action_id.as_ref().to_string(),
            kind,
            gui_revision,
            room_id,
            current_screen: current_screen.as_ref().to_string(),
        };

        let mut entries = self.inner.entries.lock().expect("gui action events lock");
        if entries.len() >= self.inner.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Return journal entries with a sequence number greater than `since_sequence`,
    /// limited to `limit` entries (clamped to 1 000).
    pub fn entries_since(&self, since_sequence: u64, limit: usize) -> Vec<GuiActionEvent> {
        let limit = limit.min(1000);
        let entries = self.inner.entries.lock().expect("gui action events lock");
        entries
            .iter()
            .filter(|e| e.sequence > since_sequence)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return the most recently assigned sequence number (0 if no entries).
    pub fn latest_sequence(&self) -> u64 {
        let val = self.inner.next_sequence.load(Ordering::Relaxed);
        if val == 0 {
            0
        } else {
            val - 1
        }
    }

    /// Return the total number of entries currently stored.
    pub fn entry_count(&self) -> usize {
        self.inner
            .entries
            .lock()
            .expect("gui action events lock")
            .len()
    }

    /// Return all stored entries (newest first for convenience).
    pub fn all_entries(&self) -> Vec<GuiActionEvent> {
        let entries = self.inner.entries.lock().expect("gui action events lock");
        entries.iter().rev().cloned().collect()
    }
}

impl Default for GuiActionEventHistory {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// GuiTestHandle — command channel for MCP → Iced
// =============================================================================

/// Handle for enqueuing GUI actions into the running Iced application.
///
/// Wraps a bounded tokio mpsc Sender shared by the MCP server and Iced app.
#[cfg(feature = "gui")]
#[derive(Debug, Clone)]
pub struct GuiTestHandle {
    sender: tokio::sync::mpsc::Sender<GuiActionRequest>,
    history: GuiActionHistory,
}
#[cfg(feature = "gui")]
impl GuiTestHandle {
    /// Create a handle backed by an existing bounded sender.
    pub fn new(sender: tokio::sync::mpsc::Sender<GuiActionRequest>) -> Self {
        Self {
            sender,
            history: GuiActionHistory::default(),
        }
    }

    /// Create a bounded GUI action channel and return its handle and receiver.
    pub fn channel(capacity: usize) -> (Self, tokio::sync::mpsc::Receiver<GuiActionRequest>) {
        let cap = capacity.clamp(1, 4096);
        let (tx, rx) = tokio::sync::mpsc::channel(cap);
        (
            Self {
                sender: tx,
                history: GuiActionHistory::default(),
            },
            rx,
        )
    }

    /// Enqueue an action, recording it in history before attempting delivery.
    pub fn enqueue(&self, request: GuiActionRequest) -> Result<(), GuiActionError> {
        use tokio::sync::mpsc::error::TrySendError;
        let action_id = self.history.record(request.clone());
        self.sender.try_send(request).map_err(|e| {
            let (error, terminal_state) = match e {
                TrySendError::Full(_) => (
                    GuiActionError::new(
                        GuiActionErrorCode::ActionQueueFull,
                        format!("GUI action queue is full (capacity: {})", self.capacity()),
                    ),
                    GuiActionState::QueueFull,
                ),
                TrySendError::Closed(_) => (
                    GuiActionError::new(
                        GuiActionErrorCode::ActionQueueClosed,
                        "GUI action channel is closed",
                    ),
                    GuiActionState::Rejected,
                ),
            };
            let _ = self.history.set_error(&action_id, error.clone());
            let _ = self.history.set_state(&action_id, terminal_state);
            error
        })
    }

    /// Return the lifecycle history shared by MCP and the Iced application.
    pub fn history(&self) -> GuiActionHistory {
        self.history.clone()
    }

    /// Return the maximum capacity of the underlying channel.
    pub fn capacity(&self) -> usize {
        self.sender.max_capacity()
    }

    /// Return whether the receiver has been dropped.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

// =============================================================================
// Tests
// =============================================================================
