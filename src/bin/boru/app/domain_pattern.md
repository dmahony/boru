# BORU-APP-002 — Domain message-routing pattern

Reference pattern for extracting cohesive subsystems out of the monolithic Iced
app shell (`app.rs`). One small demo domain (`help_overlay.rs`) implements it;
later BORU-APP-* tasks migrate the full domains following the same shape.

- Created: 2026-08-18 (BORU-ARCH-04, task t_2909ec95)
- PDF source: `Boru_Code_Improvement_Action_Plan.pdf`, BORU-APP-002
- Pairs with: `docs/architecture-refactor/architecture-boundaries.md` (ownership
  rules) and `docs/architecture-refactor/app-inventory.md` (field/message map)

## 1. The shape

Every extracted domain is a module under `src/bin/boru/app/` with four
parts:

```
DomainState    a struct that owns the domain's state (a field on `IcedChat`).
DomainMessage  an enum of messages the domain understands.
update()       a method that mutates only its own state and returns typed
               events for the shell to act on.
view()         a method that builds the domain's portion of the UI.
```

The App shell (`IcedChat`) stays the composer/router: it owns one field per
domain, routes top-level `AppMessage` variants to the right domain's `update()`,
applies the returned events (side effects), and composes `view()` output into
the screen. Startup/shutdown, top-level route switching and the global error
surface remain in the App.

```
AppMessage ──routed by update()──► DomainState::update(msg) ──► Vec<DomainEvent>
                                                                │
                                        shell applies side effects│
                                                                ▼
view() ◄── composed by App ───────────────────────────────── shell state
```

## 2. Contract

1. **State lives in the domain.** `IcedChat` holds `domain: DomainState` and
   nothing else for that domain. Do not keep a mirror copy of the same field on
   `IcedChat` (PDF §14 stop condition: same state in old and new module).
2. **Messages are domain-scoped.** `DomainMessage` is the enum the domain's
   `update()` matches on. The App keeps `AppMessage` as the single app-level
   message type and routes one-or-more variants to `domain.update(...)`.
3. **`update()` returns typed events, not bare mutation.** Side effects that
   touch other domains, the shell, or the outside world are returned as
   `DomainEvent` values. The shell's routing arm handles them. This is how a
   domain "requests" a side effect without owning unrelated state.
4. **`view()` takes only what it renders.** A domain view receives the state it
   needs (either `&self` for its own state, or an explicit parameter for the
   layer it overlays). It never reaches into another domain's fields.
5. **No cross-domain mutation.** If a domain needs another domain's data it
   emits a typed event/command to the shell, or reads a read-only context
   handle (see ARCH-002 §2). It never writes another domain's fields.

## 3. Side-effect pattern

Because `update()` is pure-ish (it mutates only the domain), every effect is
explicit:

```rust
pub fn update(&mut self, msg: HelpMessage) -> Option<HelpEvent> {
    match msg {
        HelpMessage::Toggle => {
            self.visible = !self.visible;
            Some(HelpEvent::VisibilityChanged { visible: self.visible })
        }
        HelpMessage::Close => {
            if self.visible {
                self.visible = false;
                Some(HelpEvent::VisibilityChanged { visible: false })
            } else {
                None
            }
        }
    }
}
```

The shell routing arm maps the event to a side effect (e.g. completing a
pending GUI-test action):

```rust
AppMessage::ToggleHelp => {
    if let Some(HelpEvent::VisibilityChanged { .. }) =
        self.help_overlay.update(HelpMessage::Toggle)
    {
        if let Some(action_id) = self.pending_toggle_help_action.take() {
            // shell-owned bookkeeping, not domain state
        }
    }
    iced::Task::none()
}
```

For heavier effects (I/O, networking, `iced::Task::perform`) the domain returns
an event and the shell builds the `iced::Task`; domains never own
`spawn_blocking`/service handles they do not need.

## 4. Demo: `help_overlay.rs`

`HelpOverlay` owns the chat help-overlay visibility flag (previously
`IcedChat.help_visible`). It demonstrates every part of the pattern with the
smallest possible surface: one field, two messages, one event, one view.

- `HelpOverlay { visible: bool }` — the only help-overlay state.
- `HelpMessage::Toggle | Close` — the domain messages.
- `HelpEvent::VisibilityChanged { visible }` — the typed event the shell uses
  to finish a pending GUI-test action.
- `view(chat_layer)` — builds the overlay `Stack` on top of the chat layer, or
  returns the layer untouched when hidden.

Before: `help_visible` field + `ToggleHelp` arm + `view_help()` all lived in
`app.rs` / `app/chat.rs`. After: the App routes `AppMessage::ToggleHelp` to the
domain, the chat view delegates the overlay composition to `help_overlay.view()`,
and `IcedChat` has no help-specific state other than the domain instance.

## 5. Checklist for the next extraction

1. Pick ONE domain from `docs/architecture-refactor/app-inventory.md` §2/§7.
2. Move its fields into a `DomainState` struct; replace the `IcedChat` fields
   with a single `domain: DomainState` field (move, don't copy).
3. Move the matching `AppMessage` variants into a `DomainMessage` enum where
   practical; keep the top-level `AppMessage` variants as routing surface.
4. Move the update arms into `DomainState::update()`; return typed events for
   every cross-domain/effect.
5. Move the view builders into `DomainState::view()`; compose from the App.
6. Keep the module's public surface minimal (state type + message + event + a
   couple of accessors) — no `impl IcedChat` in the domain module.
7. Tests: unit-test `update()` state transitions in the domain module; keep the
   existing app-level tests that cover the same behaviour (they must still
   pass unchanged).
8. Gate: `rb check --bin boru --features gui,video-playback,terminal`, targeted
   tests via `rb test`, `cargo fmt` on changed files, `git diff --check`.
