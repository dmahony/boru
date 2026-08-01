# Telepathy Collision Resolution Audit

Date: 2026-08-02
Scope: collision resolution for simultaneous inbound/outbound connections in boru-chat
Compared against: telepathy pattern `should_keep_new_session(local, remote, new_is_client)`
(public-key ordering tiebreaker, as described in the task; see caveat below)

## Verdict

**gap (two gaps)**

1. **Whisper layer (production): no reachable collision resolution.** The
   deterministic public-key comparator in `src/whisper/session_manager.rs` is
   dead code — `SessionManager` is never instantiated outside its own file. The
   live whisper path (`src/whisper/mod.rs`) resolves collisions with an
   unconditional `HashMap` overwrite ("last insert wins") plus a
   disconnect-eviction race. And even if the session manager were wired up, its
   comparator is a no-op: both branches of the collision handler execute the
   identical action.
2. **Gossip layer (`src/net.rs`): no deterministic convergence.** The
   origin-based heuristic returns `true` unconditionally ("newest connection
   wins"), which is order-dependent and therefore not symmetric across the two
   peers. In a simultaneous dial the two peers can each keep a *different*
   connection as their active one (~50% of event orderings). It never drops a
   connection (the loser is kept alive in `other_conns`), so messaging keeps
   working, but redundant connections persist and the peers do not converge on
   a single session.

Telepathy's public-key tiebreaker guarantees convergence because the decision
is a deterministic function of `(local, remote)` identity, which both peers
compute identically. Boru's gossip layer is *more resilient* than telepathy in
one narrow sense (it never strands the remote peer by closing the "losing"
connection), but that resilience is purchased by giving up deterministic
convergence.

---

## 1. The two implementations, as they actually are today

### 1a. Gossip layer — `src/net.rs:1134` `should_keep_new_session(ConnOrigin, ConnOrigin)`

```rust
fn should_keep_new_session(old_origin: ConnOrigin, new_origin: ConnOrigin) -> bool {
    match (old_origin, new_origin) {
        (ConnOrigin::Accept, ConnOrigin::Accept) => true,  // reconnected
        (ConnOrigin::Dial,    ConnOrigin::Dial)    => true,  // reconnected
        (ConnOrigin::Accept,  ConnOrigin::Dial)    => true,  // our dial replaces their accept
        (ConnOrigin::Dial,    ConnOrigin::Accept)  => true,  // their dial replaces our accept
    }
}
```

All four arms return `true`. The function is *effectively* "keep the newest
connection, demote the old one to `other_conns`" (`src/net.rs:1176-1185`). The
doc comment at `src/net.rs:1125-1133` claims the heuristic "prefers Dial
connections (our outgoing connections) over Accept connections", but the code
does **not** implement that preference — the `(Dial, Accept)` arm keeps the new
*Accept* over the existing *Dial*, which is the opposite of a Dial preference.
The comment summary is stale/misleading; the per-arm comments do match the code.

### 1b. Whisper layer — `src/whisper/session_manager.rs:8-20` (module doc) and `:307-335` (handler)

The module doc describes the intended telepathy-style semantics: *"the session
manager keeps the outgoing connection on the peer with the lower public-key
byte sequence and closes the incoming one … Both converge on exactly one
connection."*

The handler:

```rust
if let SessionState::Connected = entry.state {
    if self.local_public.as_bytes() < peer.as_bytes() {
        // "We have lower key → we win; close incoming."
        let wh = self.whisper_handle.clone();
        tokio::task::spawn(async move { let _ = wh.disconnect(&peer).await; });
    } else {
        // "Peer has lower key → they win; close outgoing."
        let wh = self.whisper_handle.clone();
        tokio::task::spawn(async move { let _ = wh.disconnect(&peer).await; });
    }
}
```

Both branches are byte-for-byte identical (`wh.disconnect(&peer)`), differing
only in the log message. The deterministic comparator is therefore a **no-op**:
whatever the key ordering, the same action is taken.

The live whisper actor (`src/whisper/mod.rs`) stores **one** connection per
peer in `connected: HashMap<PublicKey, Connection>` and:

- incoming: `connected.insert(remote_id, conn)` — unconditional overwrite
  (`src/whisper/mod.rs:467-473`);
- outgoing: `connected.insert(remote_id, conn)` — unconditional overwrite
  (`src/whisper/mod.rs:621`);
- disconnect event: `connected.remove(&peer)` — removes *whatever* is in the
  map for that peer, even if the disconnecting connection is an old one and the
  map currently holds a newer, live connection (`src/whisper/mod.rs:520-523`).

There is no dedup and no key-ordering decision anywhere in the production
whisper path.

---

## 2. Checkpoint answers

### 2.1 Both implementations reachable?

**Gossip: yes — full coverage.**
Both connection paths funnel into the collision handler:
- Accept path: `LocalActorMessage::HandleConnection` →
  `handle_connection(remote_id, ConnOrigin::Accept, conn)` (`src/net.rs:479-480`),
  fed by `Gossip::handle_connection` (`src/net.rs:311-317`).
- Dial path: successful dialer result →
  `handle_connection(peer_id, ConnOrigin::Dial, conn)` (`src/net.rs:533-537`).

Both call `PeerState::accept_conn` → `should_keep_new_session`
(`src/net.rs:682-693`, `1176`). No gossip connection path bypasses it.

**Whisper: no — the deterministic handler is dead code.**
`SessionManager` is referenced *only* inside `src/whisper/session_manager.rs`
(definition + unit tests). Grep across `src/`, `examples/`, `tests/` finds zero
production call sites; `git log -S "SessionManager::spawn"` shows it was never
wired anywhere since introduction in commit `722e7403`. The app uses
`WhisperBuilder` / `WhisperHandle` directly (`examples/iced_chat/main.rs:836-837`,
`examples/iced_chat/app.rs:27030-27031`, DM send at `app.rs:9809` etc.). The
production whisper collision behavior is the actor's unconditional overwrite
described in §1b.

### 2.2 Deterministic convergence?

**Gossip: no.**
The decision is a function of *event arrival order*, which is not coordinated
across peers. Simultaneous dial produces exactly two QUIC connections:
- conn1 = A→B (A: Dial, B: Accept)
- conn2 = B→A (A: Accept, B: Dial)

Each side processes its two `handle_connection` events in whatever order they
arrive. With "keep newest", each side's final active connection is whichever
event arrived *second* on that side:

| A's order        | B's order        | A active | B active | Converged? |
|------------------|------------------|----------|----------|------------|
| conn1(Dial) then conn2(Accept) | conn1(Accept) then conn2(Dial) | conn2 | conn2 | yes |
| conn2(Accept) then conn1(Dial) | conn2(Dial) then conn1(Accept) | conn1 | conn1 | yes |
| conn1(Dial) then conn2(Accept) | conn2(Dial) then conn1(Accept) | conn2 | conn1 | **no** |
| conn2(Accept) then conn1(Dial) | conn1(Accept) then conn2(Dial) | conn1 | conn2 | **no** |

Two of the four equally likely orderings leave each side active on a different
connection. This is not a liveness failure — the demoted connection's sender is
kept alive in `other_conns` (`src/net.rs:1182-1183`, comment at `1114-1120`) so
the remote's active connection is never closed by us, and both recv loops keep
delivering (protocol-level dedup in the PlumTree message cache, 30s retention,
`src/proto/plumtree.rs:348`, suppresses duplicate broadcast delivery). The cost
is that both connections stay up indefinitely as a peer-pair, and the active
connection is not the same on both sides.

**Whisper: no (and the intended mechanism is broken).**
Even ignoring that the session manager is dead code, its comparator can't
converge because both branches call `disconnect(&peer)` (see §1b). And the live
actor's one-slot `HashMap` means whichever of the two connections registers
*last* silently replaces the other in the map while both read loops run — there
is no agreed winner.

### 2.3 Edge cases

| Case | Gossip layer (`net.rs`) | Whisper layer (production) |
|------|--------------------------|------------------------------|
| Same-origin reconnection `(Accept,Accept)` / `(Dial,Dial)` | Newest wins, old demoted. Both peers see the same new conn → converges. OK. | New insert overwrites old slot; both readers run; double `Connected` event to the UI. Works, but state blurs. |
| Simultaneous dial + accept | Non-convergent per §2.2 — each side can keep a different active conn; both stay alive. Functional, redundant. | Last insert wins; both readers run; whichever conn disconnects first evicts the *current* slot (race, §2.4). |
| Rapid connect/disconnect flapping | Each new conn replaces active; demoted conns accumulate in `other_conns` until their loops finish. When the *active* conn dies, `PeerDisconnected` is emitted (`src/net.rs:741-747`) and the peer redials; **there is no promotion logic** from `other_conns` back to active — a still-alive backup is wasted and a fresh dial is made while the backup lingers. Self-healing but churny. | Each `Disconnected` removes the slot; next send redials. Same churn. |

Additional whisper-specific race (§2.4): a disconnect event for an *old*
connection removes the *new* (live) connection from the map.

### 2.4 Gap: `new_is_client` (telepathy) vs `ConnOrigin` (boru)

**Yes — there are concrete cases where the two patterns resolve differently.**

Telepathy's `new_is_client` identifies which side initiated the new connection,
but the *tiebreaker* is public-key ordering, a function of `(local, remote)`
identity. Both peers evaluate the same function and therefore converge.

Boru's gossip layer never looks at identity. Because the same physical
connection is `Dial` on one side and `Accept` on the other, any origin-based
rule — Dial-preferred, Accept-preferred, *or* newest-wins — gives the two peers
instructions that can conflict. In the simultaneous-dial case:

- Telepathy: both peers pick the same connection (say, the one dialed by the
  lower-key peer). Converged, one connection survives.
- Boru: each side keeps the connection whose event arrived second on that
  side. In orderings 3 and 4 of the table in §2.2, the peers pick *different*
  connections. Both survive (keep-alive), but there is no single converged
  session.

Concrete reproduction (orderings 3/4): two nodes A and B with direct addresses,
each simultaneously dialing the other (e.g. both send DMs at the same instant,
or both receive a gossip `Join` for each other and dial). If A's dial
completes before its accept arrives while B's accept arrives before its dial
completes, A stays active on A→B and B stays active on B→A. Both connections
remain open; `debug!("active send connection closed")` never fires on either
side; the redundancy persists for the lifetime of the connections.

### 2.5 Documentation gap

- `src/net.rs:1125-1133` — the summary comment claims a "Dial > Accept"
  preference that the code does not implement (all arms return `true`).
  Maintainers reading the summary will mispredict `(Dial, Accept)` behavior.
- `src/whisper/session_manager.rs:1-20` — module docs describe deterministic
  lower-key-wins semantics, but the code (a) is not wired anywhere and (b) does
  not actually implement the described semantics (both branches disconnect).
- `docs/protocol-layers.md:184` — states "`session_manager` owns whisper
  reconnect/backoff and collision resolution", which is not true of the
  production binary.
- There is no existing doc explaining the `other_conns` keep-alive design or
  the convergence properties of the gossip collision handler. This file is the
  first.

---

## 3. Reproduction steps

### Gap A — whisper collision: live path has no resolution + eviction race

1. Start two boru instances, A and B, on a LAN.
2. From A, send a DM to B; *immediately* (within ~1s) from B, send a DM to A.
   This creates conn1 (A→B) and conn2 (B→A) with both sides seeing an incoming
   and an outgoing.
3. Observe (trace logs): each side emits two `WhisperEvent::Connected` for the
   same peer (actor lines `src/whisper/mod.rs:470` and `:622`); the UI pushes
   two "[Whisper] Connected" system messages (`examples/iced_chat/app.rs:10766-10772`).
4. From B, call `whisper_handle.disconnect(&a)` (or revoke B→A authorization,
   which routes through `Cmd::Disconnect`, `src/whisper/mod.rs:449-459`). This
   closes conn1 on B's side.
5. On A: conn1's read loop sees the disconnect and emits
   `ConnectionEvent::Disconnected(A)`, which removes the *current* slot from
   A's map (`src/whisper/mod.rs:520-523`) — that slot is conn2, the live
   incoming connection. A now has no map entry for B even though conn2 is
   healthy; A's next DM dials a fresh connection.
6. There is no deterministic winner at any point: the outcome is purely a
   function of arrival/close order.

### Gap B — gossip collision: non-convergent simultaneous dial

1. Run two gossip nodes A, B with known direct addresses.
2. Trigger simultaneous dial: on A, `Join` a topic bootstrapping on B while on
   B, `Join` the same topic bootstrapping on A, at the same moment (or both
   call `connect`/dial each other concurrently).
3. Inspect with tracing (`debug` level, filter `conn`): the two `handle_connection`
   events arrive in a different order on A than on B in ~50% of runs.
4. When the orders differ (§2.2 table rows 3–4), both nodes log
   "session collision: rejecting…" *or* "dial successful" + "connection
   established" for their own outgoing conn, and each node's `PeerState` has a
   different `active_conn_id`. Neither node ever logs the other connection
   closing; both connections remain open (`other_conns` keeps the senders
   alive). The peer pair is stuck with two simultaneous QUIC connections.
5. Expected (telepathy): both nodes converge on one connection, and the
   "losing" connection is closed.

---

## 4. Why boru's gossip layer is *partly* more sophisticated than telepathy

The `other_conns` design (`src/net.rs:1114-1120`, `1182-1183`) is genuinely more
careful than telepathy's "close the loser" approach: closing the losing
connection on one side can tear down the connection the remote peer has just
promoted to *its* active session. Boru avoids that footgun by keeping the
demoted sender alive until the remote side closes the connection itself. The
trade-off is that the collision decision is not deterministic, so the peers
never converge on a single session and redundant connections accumulate.

A fix that preserves the keep-alive safety *and* adds telepathy's convergence
would be: keep the demoted connection alive (as today), but make the *choice of
which connection becomes active* a deterministic function of the public keys —
e.g. when `(old, new)` origins are mixed, prefer the connection dialed by the
lower-key peer — instead of unconditionally preferring the newest arrival.

## 5. Caveat on telepathy source

The telepathy example (`should_keep_new_session(local, remote, new_is_client)`)
is not present in the current `n0-computer/iroh` tree (examples were removed
during the repo restructure; confirmed absent at `main` and at v0.31.0/v1.0.0
tag trees). The comparison above uses the pattern stated in the task body and
iroh's documented connection-collision approach; the exact current telepathy
source could not be fetched for byte-level comparison.

## 6. Files and lines referenced

- `src/net.rs:311-317` — `Gossip::handle_connection` (accept entry)
- `src/net.rs:479-480` — Accept path → `handle_connection(..., ConnOrigin::Accept, ...)`
- `src/net.rs:533-537` — Dial path → `handle_connection(..., ConnOrigin::Dial, ...)`
- `src/net.rs:682-693` — `PeerState` entry / `accept_conn`
- `src/net.rs:1106-1122` — `PeerState` + `other_conns` rationale
- `src/net.rs:1125-1151` — `should_keep_new_session` (all-`true`; stale summary comment)
- `src/net.rs:1176-1193` — keep-new / reject-new logic
- `src/net.rs:1242-1245` — `ConnOrigin` enum
- `src/whisper/mod.rs:449-459` — `Cmd::Disconnect` (closes current slot)
- `src/whisper/mod.rs:467-473` — incoming insert (unconditional overwrite)
- `src/whisper/mod.rs:520-523` — `Disconnected` eviction race
- `src/whisper/mod.rs:621-622` — outgoing insert (unconditional overwrite)
- `src/whisper/session_manager.rs:13-20` — intended (dead) semantics
- `src/whisper/session_manager.rs:307-335` — collision handler (identical branches)
- `examples/iced_chat/app.rs:10766-10772` — app's `Connected` handler (no collision logic)
- `examples/iced_chat/main.rs:836-837, 946` — whisper wiring (protocol handler only)
- `docs/protocol-layers.md:184` — stale claim about `session_manager`
- `src/proto/plumtree.rs:348` — message cache retention (protocol dedup)
