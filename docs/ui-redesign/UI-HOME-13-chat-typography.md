# UI-HOME-13 — Figtree for Chat Messages and the Composer

- Task: `t_2eb8d1d4` (UI-HOME-13)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-13 card)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. Chat timeline (message body, sender names, timestamps/status,
  system notices, reactions, link previews) and the shared composer now use the
  central `fonts::TypeRole` Figtree roles from UI-HOME-11. Chat behaviour is unchanged.

## 1. What was delivered

Applied the central semantic typography roles (UI-HOME-11 `TypeRole`) to the chat
screen in `examples/iced_chat/app.rs`:

| Element | Before | After (role) |
|---|---|---|
| Message body (incoming/outgoing) | iced default font (Source Sans 3) at `chat_text_size` | `TypeRole::ChatMessage` → Figtree Regular 400, + `LineHeight::Relative(1.45)` |
| Sender name (label, incl. online-icon and failed-retry buttons) | `source_sans(Semibold)` at TYPO_SM (14) | `TypeRole::ChatSender` → Figtree SemiBold 600 @ 14 px |
| Timestamp / delivery status (`HH:MM · state`) | default font at TYPO_XXS (12) | `TypeRole::ChatMetadata` → Figtree Regular 400 @ 12 px |
| System notices (plain) | default font at TYPO_XS, muted, centred | Figtree Regular (`TypeRole::ChatMessage`), muted/centred layout kept → stays semantically distinct |
| Link preview card (title, description, urls) | default font | Figtree Regular (`TypeRole::ChatMessage`) |
| Reactions line | default font | Figtree Regular (`TypeRole::ChatMessage`) |
| Pending upload/processing notices | default font | Figtree Regular (`TypeRole::ChatMessage`) |
| Composer input + placeholder | default font at `chat_text_size` | `TypeRole::ComposerText` → Figtree Regular 400 |
| Technical values (peer IDs in chat header/options/search, `jetbrains_mono`) | unchanged | unchanged (JetBrains Mono retained) |

Sizes follow the app's established user-facing controls: the message body and
composer keep the user-configurable `chat_text_size` setting (default TYPO_SM = 14 px;
the `TypeRole` canonical default is 15 px per the UI-HOME-11 handoff — the live size
deliberately stays user-controlled). Sender 14 px and metadata 12 px match the role
canonical sizes exactly.

Message line height: `text::LineHeight::Relative(1.45)` applied to the message body
(plan mapping ~1.45–1.5). Iced 0.14 renders this via font metrics; bubbles grow
correctly with wrapped lines.

## 2. Changed shared components

All chat-screen components live in `app.rs` (`view_chat_log`, `view_composer`); no
shared `ui_components.rs` chat primitive needed a change (date separators keep the
`Typography::Timestamp` token; system chips in `ui_components.rs` are only used by the
component gallery, not the live timeline).

- `examples/iced_chat/app.rs`
  - message body: 3 sites (plain body, text segment, URL segment) → Figtree + line-height
  - sender label: 4 sites (remote w/ presence icon, remote plain, local failed-retry, local plain) → Figtree SemiBold
  - timestamp/metadata → Figtree
  - system message text → Figtree (muted/centred kept)
  - link preview title/description/url/url-preview/loading → Figtree
  - reactions line → Figtree
  - pending image/file processing + uploading notices → Figtree
  - composer `text_input` → Figtree (`TypeRole::ComposerText`)
  - + 3 new regression tests (see §5)

## 3. Message-type test report (manual/visual, evidence screenshots)

Driven against the built binary under Xvfb (1280×800) with a seeded direct
conversation (`scripts/seed_two_instances.py` + hand-written `chat_history.json`
replay, deterministic) and a live two-instance MCP run for the initial capture.

| Test | Result | Evidence |
|---|---|---|
| Short message | PASS — single-line bubble, clean edges | `incoming_outgoing_system.png` (URL test bubble, “Another incoming line…”) |
| Multi-line message | PASS — wraps to multiple lines, bubble grows, no clipping | `incoming_outgoing_system.png` (“Multi-line outgoing message: … Line two with a URL: … Line three with emoji …”) |
| Long unbroken text | PASS — `Wrapping::WordOrGlyph` breaks the long word at the bubble cap | same screenshot (`longunbrokenwordwithoutspaces…`) |
| Emoji | PASS — renders via font fallback (Figtree has no emoji glyphs; iced fontdb falls back), layout stable | same screenshot (🎉/🚀) |
| URLs | PASS — URL text wrapped and clickable (segmented row); live run also produced a link-preview card | same screenshot (URL visible; link preview “Example Domain” in the earlier live capture) |
| System messages | PASS — centred, muted, small; family Figtree but visually distinct from user bubbles | same screenshot (“Peer joined”, “… is now known as BPeer”) |
| Sender names | PASS — Figtree SemiBold, friend label “BPeer” with presence icon | same screenshot |
| Timestamps / read state | PASS — “18:26 Read” formatting unchanged (delivery/read-state presentation intact) | same screenshot |
| Composer with multiple lines | PASS — pasted 3-line content is preserved in the composer value; iced 0.14 `TextInput` is a fixed-height single-line widget (it does not grow), so the content renders on one line with no clipping or layout breakage; multi-line *messages* grow in bubbles | `composer_multiline.png` |
| No bubble / composer clipping | PASS — all wrapped lines and pasted composer content fully visible | both screenshots |
| Replies / reply previews | N/A — this app has no reply feature (verified: no reply/reply-preview code exists in `examples/iced_chat/`) | — |

## 4. Verification

- Build: `cargo build --example boru --features gui` → OK (exit 0).
- Tests: `cargo test --example boru --features gui` → 844 passed, 0 failed.
  This includes the 3 new UI-HOME-13 regression tests:
  - `chat_timeline_uses_type_role_figtree_roles` — view_chat_log must use
    `TypeRole::ChatMessage/Sender/Metadata` + `LineHeight::Relative(1.45)` and must
    NOT use Source Sans SemiBold for sender labels.
  - `composer_uses_type_role_composer_text_font` — view_composer must use
    `TypeRole::ComposerText` and keep the user-configurable text size.
  - `chat_roles_map_to_figtree_at_plan_sizes` — role → family/weight/size mapping
    (ChatMessage 15/400, ChatSender 14/600, ChatMetadata 12/400, ComposerText 15/400,
    TechnicalValue 12/400 JBM).
- Evidence screenshots (Xvfb + `import`, OCR-verified):
  - `docs/ui-redesign/evidence/t_2eb8d1d4/t_2eb8d1d4_incoming_outgoing_system.png`
  - `docs/ui-redesign/evidence/t_2eb8d1d4/t_2eb8d1d4_composer_multiline.png`

## 5. Files changed (this task only)

- `examples/iced_chat/app.rs` — chat timeline + composer typography migration + 3 tests
- `docs/ui-redesign/UI-HOME-13-chat-typography.md` — this report
- `docs/ui-redesign/evidence/t_2eb8d1d4/` — evidence screenshots

## 6. Remaining risks / notes for downstream cards

- **Concurrent work in the shared checkout (IMPORTANT):** task `t_7595a388`
  (UI-HOME-03 card-shell work) has uncommitted changes to `card_shell.rs`,
  `design_tokens.rs`, `fonts.rs` plus its own evidence dir. This task made a
  **one-line compile fix** to `fonts.rs` (`type_role_text_lh_builds_text_widget`
  smoke test: replaced the typed-`Element` conversion with `let _ = widget;`, which
  does not type-check under the fallback renderer) so the shared tree's test build
  compiles. That fix is part of the other task's uncommitted `fonts.rs`, NOT this
  commit. The other task should be aware it is carrying it.
- The composer is a single-line iced `TextInput` (fixed height in iced 0.14.2);
  “composer growth with multiple lines” from the plan does not map to iced 0.14
  widget semantics. Not changed here (out of scope — font application only). If
  multi-line composer editing is desired, that is a separate feature task (e.g.
  swapping to a multi-line editor widget), not a typography change.
- Message body size stays at the user-configurable `chat_text_size` (default 14 px),
  rather than the role canonical 15 px, so the existing Settings text-size control
  keeps working (deliberate; documented deviation from the plan's “~15–16 px” target).
- Link-preview cards in the seeded-history evidence render as plain wrapped URL text
  (history replay does not re-run link-preview fetch); a live two-instance run
  produced a real preview card (earlier capture). No code change.
- No font files are exposed in reports/artifacts (OFL records live in-repo from
  UI-HOME-11).
