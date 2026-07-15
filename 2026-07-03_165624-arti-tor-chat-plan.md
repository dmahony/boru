# Arti Tor Integration for iroh-gossip Chat Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Add a Tor bootstrap gate and pre-chat connection status display to the chat example, while being explicit that the existing iroh transport is not automatically Tor-routed.

**Architecture:**
Use Arti only as a readiness gate and status source before any chat/gossip activity starts. Bootstrap Tor first, surface its progress and blockage details to the user, and only then decide whether to proceed into the iroh chat path. Do not pretend this makes iroh traffic anonymous: iroh 1.x still uses its own QUIC/relay transport, so strict Tor-only operation requires a separate Tor-compatible transport redesign.

**Tech Stack:**
Rust 1.91, `arti-client 0.44.x`, existing `iroh 1.x` / `iroh-gossip`, Tokio, Clap, tracing.

---

## Findings That Drive the Design

- `arti-client` already exposes exactly the status hooks we need:
  - `TorClient::create_bootstrapped(config).await?`
  - `TorClient::bootstrap_status()`
  - `TorClient::bootstrap_events()`
- `BootstrapStatus` has human-readable progress/blockage formatting and a readiness predicate:
  - `ready_for_traffic()`
  - `Display` prints percent + connection/directory status, or blockage text.
- `iroh` is a QUIC / peer-connection stack with relay and hole-punching features; its HTTP proxy support only covers HTTP(S) traffic, not the core peer transport.
- Therefore, Arti can gate startup and provide UX, but it does not by itself make the current iroh gossip path Tor-only.

---

## Plan

### Task 1: Add Arti as an explicit dependency and pin the version

**Objective:** Make the Tor client available to the chat example with a version that matches the repo's Rust MSRV.

**Files:**
- Modify: `Cargo.toml`

**Recommended change:**
- Add `arti-client = "0.44"` to dependencies.
- If the implementer wants feature minimalism, prefer an explicit feature list only after checking what Arti needs for the chosen runtime; otherwise use the crate defaults.

**Why:**
- `arti-client 0.44.x` is current, documented, and advertises Rust 1.91 compatibility, which matches this crate.

**Verification:**
- Run `cargo tree -e normal -p arti-client`
- Run `cargo check --examples --features examples`

---

### Task 2: Build a Tor bootstrap helper that prints progress before any gossip code runs

**Objective:** Create one startup path that always connects to Tor first and exposes status text before the user can chat.

**Files:**
- Modify: `examples/chat.rs:1-185`
- Optional new file if you want shared code: `examples/tor.rs`

**Implementation notes:**
1. Create the Arti client before any `iroh::Endpoint`, `Gossip::builder().spawn`, `Router::builder().spawn`, `subscribe_and_join`, or `broadcast` call.
2. Use `TorClient::builder().config(TorClientConfig::default()).create_unbootstrapped()` if the UX should stream live status, then call `bootstrap()` separately.
3. Spawn a small status task that reads `bootstrap_events()` and prints `BootstrapStatus` updates until `ready_for_traffic()` is true.
4. Show a stable line when Tor is ready, for example:
   - `Connecting to Tor...`
   - `Tor status: 31%: ...`
   - `Tor ready; proceeding to chat`
5. On failure, print the blockage kind and message, then exit without touching iroh networking.

**Why this shape:**
- `create_bootstrapped()` is simpler, but it hides the progress that the user explicitly asked to see.
- `bootstrap_events()` gives enough detail to explain why Tor is slow or blocked without exposing unnecessary identity data.

**Do not print:**
- Secret key material
- Full endpoint addresses
- Relay keys / tokens
- Any DNS, SOCKS, or system-path information not required for the user to understand readiness

**Verification:**
- `cargo run --example chat -- --help`
- `cargo run --example chat -- open`
- Confirm the first observable network-facing activity is Tor bootstrap output, not iroh endpoint setup

---

### Task 3: Gate all gossip/chat activity behind successful Tor readiness

**Objective:** Ensure no chat traffic, topic join, or peer exchange starts before Tor reports usable connectivity.

**Files:**
- Modify: `examples/chat.rs:77-185`
- Possibly modify: `examples/setup.rs:1-21` if the setup example is user-facing and also opens networking

**Implementation notes:**
1. Keep all network setup in a post-bootstrap block.
2. Do not construct the iroh endpoint, router, or gossip instance until Tor bootstrap succeeds.
3. Do not call `subscribe_and_join()` or print tickets until Tor is ready.
4. If the join path fails after Tor is ready, shut down cleanly and report the error.

**Leak-prevention checklist:**
- No iroh relay connection attempts before Tor readiness
- No peer ticket exchange before Tor readiness
- No chat messages sent before Tor readiness
- No bootstrapping bypass path hidden behind `no_relay`

**Verification:**
- Add or update tests if the example code is covered by compile-time tests.
- Manually confirm startup order with `cargo run --example chat -- open`.

---

### Task 4: Decide the truth about iroh transport leakage and encode it in the UX

**Objective:** Avoid claiming Tor anonymity that the transport does not provide.

**Files:**
- Modify: `examples/chat.rs:24-156`
- Modify: `README.md:15-76`

**Recommended decision:**
- Treat Arti bootstrap as a precondition, not as proof that iroh traffic is Tor-routed.
- Add an explicit on-screen warning that the current iroh transport still uses its own networking stack.
- If the product requirement is strict Tor-only transport, make the example fail closed here and require a separate Tor-compatible transport implementation before chat can proceed.

**UX requirements for the connection detail display:**
- Show human-readable Tor bootstrap progress and blockage status.
- Show a clear “Tor ready” marker before any chat UI/input prompt.
- Avoid overpromising: distinguish “Tor is ready” from “all subsequent traffic is Tor-routed”.
- If strict Tor-only is not yet implemented, print a one-line warning before entering chat mode.

**Suggested warning copy:**
- “Tor bootstrap succeeded. This example’s existing iroh transport is not yet Tor-routed, so chat traffic may still leave outside Tor unless a Tor-backed transport is added.”

**Why this matters:**
- Without this distinction, the UI would create a false sense of anonymity.

**Verification:**
- `cargo run --example chat -- open`
- Confirm the warning appears before any prompt to type messages

---

### Task 5: Remove or quarantine the current secret-key leak in the chat example

**Objective:** Stop printing endpoint secret material while adding a Tor bootstrap flow.

**Files:**
- Modify: `examples/chat.rs:91-100`
- Optionally update: `README.md:19-76`

**Recommended change:**
- Stop echoing the raw secret key to stdout by default.
- If identity reuse is important, replace it with an explicit `--show-secret-key` or `--save-secret-key` mode, or write the key to a restricted file path instead of the terminal.

**Why this belongs in the Tor work:**
- A Tor-aware startup that still prints the private endpoint key is not privacy-safe.

**Verification:**
- `cargo run --example chat -- open`
- Confirm no secret key appears in the terminal output

---

### Task 6: If strict Tor-only transport is required, stop and redesign the data plane instead of patching around it

**Objective:** Make the security boundary explicit.

**Files:**
- Likely new modules under `src/` for a Tor-backed rendezvous layer, or a new example if the design is only illustrative.

**Recommended architecture if you need real Tor-only behavior:**
- Use Arti to reach a TCP rendezvous service over Tor.
- Move ticket exchange / peer discovery onto that service.
- Keep the existing iroh gossip data plane disabled until there is a Tor-compatible transport or custom transport implementation.
- Do not claim the existing `iroh::Endpoint` / `RelayMode` path is Tor-safe just because Arti bootstrapped first.

**Why:**
- iroh's current transport model is the incompatible part, not Arti's bootstrap flow.
- A one-line dependency swap will not change that.

**Verification:**
- If the redesign is implemented, add an integration test that fails if any non-Tor socket is opened before the Tor rendezvous is established.

---

## Files Likely to Change

- `Cargo.toml`
- `examples/chat.rs`
- `examples/setup.rs` if it also opens network connections for users
- `README.md`
- Potentially new `examples/tor.rs` or `src/tor.rs` helper if the bootstrap code should be shared

---

## Verification Commands

Run these after implementation:

```bash
cargo check --examples --features examples
cargo run --example chat -- --help
cargo run --example chat -- open
cargo run --example setup
cargo test
```

If the implementer adds a Tor-only redesign, add at least one targeted test that exercises startup ordering and one manual smoke test showing that the Tor bootstrap output appears before any chat prompt.

---

## Acceptance Criteria

- Tor bootstrap happens before any gossip/chat exchange.
- The user sees Tor progress/blockage details before chat starts.
- The implementation does not print secret key material.
- The design explicitly states that the existing iroh transport is not automatically Tor-routed.
- If strict Tor-only behavior is required, the plan refuses to fake it and points to a separate transport redesign.
