# RustDesk Reference Policy for Boru Screen Sharing

Status: **binding policy** for the Boru screen-sharing work (BORU-SS task chain,
phases 0-14 of `Boru_RustDesk_Reference_Screen_Sharing_Tasks.pdf`).

Origin: Phase 0 / Task 0.1 ("Add a RustDesk reference policy") of
`Boru_RustDesk_Reference_Screen_Sharing_Tasks.pdf` (attached to kanban task
t_2d8629a8). First published in commit 6c22bff8 (archived BORU-SS chain) and
refined by the canonical BORU-SS-01 chain (t_3d3d896e). The PDF's **Agent Rule**
is binding on every agent in the chain: *"Do not optimize by copying RustDesk.
When RustDesk exposes an edge case or useful technique, write a Boru
requirement, find the relevant platform/API documentation, and implement the
behavior independently. Preserve Boru's MIT/Apache-2.0 licensing flexibility."*

RustDesk is used as an **engineering reference only**. Boru screen sharing is a
native Boru subsystem built from Boru-owned code, upstream platform APIs,
official documentation, and permissively licensed libraries. Boru's source
licensing stays **MIT/Apache-2.0**. This document defines what may be inspected,
what is prohibited, and the review gate every screen-sharing change must pass.

## 1. What may be inspected

RustDesk (AGPL-3.0) may be studied as a **black-box behavioral reference** for
engineering ideas, edge cases, and platform behavior. Permitted areas of study:

- **Architecture** — how a remote-desktop product is structured (capture,
  encoding, transport, rendering) at a conceptual level.
- **Behavior** — observable user-visible and system-visible behavior: how the
  product reacts to monitor changes, resolution changes, cursor movement,
  disconnects, reconnects, permission denial, and degraded networks.
- **UX** — interaction patterns: share initiation, source selection, consent
  flow, persistent "remote control active" indicators, stop/revoke affordances.
- **Failure modes** — what breaks and how it is presented/handled (permission
  failures, unplugged monitors, codec errors, latency buildup, stale frames).
- **Protocol concepts** — message roles and sequencing (offer/accept/reject,
  keyframe requests, quality updates, separate control vs. media channels).
  Study the *concept*, not the implementation.
- **Capture strategy** — which platform capture mechanisms exist and why one
  might be chosen (WinRT Graphics Capture, xdg-desktop-portal ScreenCast +
  PipeWire, X11/XWayland fallback).
- **Performance ideas** — frame dropping over latency buildup, dirty-region /
  damage-aware capture, cursor-shape optimization, adaptive quality.

Observations are recorded as **behavioral requirements** in Boru terms (see
BORU-SS Task 1.2). RustDesk source text and implementation details must not be
reused.

## 2. Explicit prohibitions

The following are **never** permitted when working on Boru screen sharing:

- **Copying source** — copying, pasting, or closely reproducing RustDesk source
  code, in whole or in part, into any Boru file.
- **Line-for-line translation** — translating RustDesk functions or modules
  from their implementation language into Rust (or any language), even with
  renamed identifiers.
- **Mechanical porting / close reproduction** — porting RustDesk algorithms,
  data structures, or control flow such that the result is substantially
  derived from AGPL-covered code.
- **Copying creative expression** — copying RustDesk comments, doc strings,
  tests, test vectors, constants, or other content carrying creative
  expression.
- **Importing AGPL crates/modules** — adding RustDesk AGPL crates, modules,
  or any GPL/AGPL dependency to Boru's compiled dependency graph without
  explicit review. Boru's dependency graph must stay permissively licensed.
- **Tunneling an external remote-desktop product** — screen sharing must
  remain a native Boru subsystem (capture → frame normalization → encoder →
  Boru screen-share protocol → Iroh stream → decoder → Iced render surface).
  Never wrap or tunnel RustDesk (or another external remote-desktop product)
  into Boru's UI or session.

If a piece of RustDesk behavior is genuinely useful, **do not copy it** —
convert it into a Boru requirement, find the relevant platform/API
documentation, and implement the behavior independently (section 4).

## 3. Independent-source citation requirement

Every Boru screen-sharing implementation decision must cite the **independent
source** it is based on — upstream platform APIs, official documentation, or
permissively licensed libraries. Examples of acceptable sources:

- **Windows capture:** Microsoft WinRT Graphics Capture / Direct3D
  documentation (`learn.microsoft.com/windows/...`).
- **Wayland capture:** xdg-desktop-portal ScreenCast/RemoteDesktop
  specifications (`flatpak.github.io/xdg-desktop-portal/`), PipeWire
  documentation (`docs.pipewire.org`).
- **X11 capture/input:** X11 protocol documentation, `x11rb` crate docs.
- **Encoding:** OpenH264 API documentation (`github.com/cisco/openh264`,
  `openh264.org`).
- **Iroh transport:** Iroh docs and API reference (`docs.iroh.computer`,
  `docs.rs/iroh*`).
- **Rust std/ecosystem:** `doc.rust-lang.org`, `docs.rs` for permissively
  licensed crates already in the dependency graph.

Workflow when RustDesk exposes an edge case or useful technique:

1. Write a Boru requirement describing the desired behavior in Boru terms.
2. Find the relevant platform/API documentation or permissively licensed
   library that provides it.
3. Implement independently against that source.
4. **Cite the independent source** in the commit message and/or the PR
   description for each decision.

A Boru implementation decision that cannot be traced to an independent source
must be flagged for review before merging.

## 4. Licensing guardrails

- Boru source remains **MIT/Apache-2.0** (see `Cargo.toml` `license`).
- **No GPL/AGPL dependencies** enter the compiled Boru dependency graph without
  explicit review (see BORU-SS-02 for the automated licence gate).
- Third-party notices for assets and redistributed native binaries are kept
  **separate** from Boru's MIT/Apache-2.0 source licensing.
- Reuse Boru/Iroh encrypted P2P sessions, NAT traversal, relay fallback, and
  existing session security. Preserve existing networking, chat, file
  transfer, video, tunnel, lobby, room, and persistence behaviour.

## 5. Screen-sharing PR checklist

No screen-sharing-specific PR template exists under `.github/` (only
`dependabot.yml`, `ISSUE_TEMPLATE/`, `release-drafter.yml`, `workflows/`), so
the checklist item lives in the general PR review gate that every Boru PR passes:
the **Pull request checklist** in `docs/CONTRIBUTING.md`. PRs are reviewed on
GitHub; reviewers see that checklist when opening the PR.

The following item is part of the `docs/CONTRIBUTING.md` Pull request checklist
and **must** be confirmed in every screen-sharing PR (any change touching the
`screen-sharing` cargo feature or `src/screen_share/`):

> - [ ] **No RustDesk code was copied.** This change contains no RustDesk
>       (AGPL-3.0) source code, no line-for-line translations or mechanical
>       ports of RustDesk code, and no copied comments/tests/constants from
>       RustDesk. Every implementation decision is based on an independent
>       source (platform API docs, official specifications, permissively
>       licensed libraries) cited in the PR description. No GPL/AGPL
>       dependency was added to the compiled graph.

This matches the Definition of Done requirement: *"A code-review checklist
confirms no RustDesk AGPL code was copied or mechanically translated."*

## 6. Enforcement and review

- Every screen-sharing PR must pass the checklist in section 5 before merge.
- Commits that add screen-sharing code must state the independent source for
  each notable decision (or reference the PR description that does).
- Reviewers are expected to reject any change that appears to reproduce
  RustDesk implementation code, even if renamed or reformatted.
- Automated licence gating (cargo-deny or equivalent) is tracked separately
  in BORU-SS-02.
