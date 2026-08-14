# Third-Party Notices — Boru

This file is the licensing manifest for **Boru** (this repository). It records
the project's own copyright and licence, and the individual licences and
notices of every third-party component Boru builds on or bundles, including
the upstream crates Boru modifies.

The document is deliberately split into **source licensing** (sections 1–3:
what Boru's own code and its compiled Rust dependencies are licensed under)
and **non-source third-party material** (sections 4–5: bundled assets and
redistributed native binaries, which are separate works with their own
licence obligations and are NOT covered by Boru's MIT/Apache-2.0 licensing).
Section 6 documents the automated licence gate that enforces this policy.

## 1. Boru source licensing

- **Copyright © 2026 Daniel Mahony** — all Boru modifications and additions.
- Base project: the `iroh-gossip-chat` example from the n0 team's `iroh`
  repository, **Copyright 2023 N0, INC.**
- Licence: **MIT OR Apache-2.0** (dual) — see `LICENSE-MIT` and
  `LICENSE-APACHE` at the repository root; declared in `Cargo.toml`
  (`license = "MIT/Apache-2.0"`).
- Repository: https://github.com/dmahony/boru

Boru is a modified fork of n0's `iroh-gossip-chat`. All modifications and
additions are **Copyright © 2026 Daniel Mahony** and are distributed under the
same dual licence as the base project.

## 2. Patched crates — Boru modifications of upstream crates

Boru vendors modified copies of upstream crates under `patched/` and
`noq-proto-patched/`, wired in via `[patch.crates-io]` in `Cargo.toml`. The
modifications are **by Daniel Mahony** and are licensed under the same terms
as each upstream crate. Every patched directory retains its upstream licence
text.

| Crate | Directory | Upstream licence | Upstream copyright |
|---|---|---|---|
| `iroh` | `patched/iroh` | MIT OR Apache-2.0; BSD-3-Clause for tailscale-derived parts (`LICENSE-BSD3`) | n0 team |
| `iroh-dns` | `patched/iroh-dns` | MIT OR Apache-2.0 | n0 team |
| `iroh-relay` | `patched/iroh-relay` | MIT OR Apache-2.0; BSD-3-Clause for tailscale-derived parts (`LICENSE-BSD3`) | n0 team |
| `irpc` | `patched/irpc` | MIT OR Apache-2.0 | N0, INC. |
| `irpc-iroh` | `patched/irpc-iroh` | Apache-2.0 OR MIT | Rüdiger Klaehn, n0 team |
| `mainline` | `patched/mainline` | MIT | raptorswing (2021) |
| `n0-mainline` | `patched/n0-mainline` | MIT OR Apache-2.0 | raptorswing (2021) |
| `iced_tiny_skia` | `patched/iced_tiny_skia` | MIT | iced contributors |
| `noq-proto` | `noq-proto-patched` | MIT OR Apache-2.0 | quinn developers (2018–2025), N0, Inc. (2025) |

## 3. Rust dependency tree (source dependencies)

The complete set of crates.io dependencies, their licence expressions, and
the allowed-licence policy are validated by **cargo-deny** — see `deny.toml`
(allow list) and `Cargo.lock` (full inventory). Allowed licences are:

Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause,
BSL-1.0, CC0-1.0, CDLA-Permissive-2.0, IJG, ISC, MIT, MPL-2.0, NCSA,
Unicode-3.0, Unlicense, Zlib.

Everything else — in particular the copyleft licences GPL/AGPL/LGPL/GFDL and
any licence not listed above — is **rejected** by the gate. The few crates
whose licence is not machine-declarable are handled with an explicit
clarification (`ring` in `deny.toml`).

## 4. Bundled assets (not part of the compiled binary)

| Component | Location | Licence | Notice |
|---|---|---|---|
| Fonts (Figtree, Raleway, JetBrains Mono, Archivo, IBM Plex Sans, Public Sans, Inter Tight) | `examples/iced_chat/fonts/` | SIL OFL-1.1 | `examples/iced_chat/fonts/THIRD_PARTY_NOTICES.md` (exact versions and sources) |
| Papirus file-type icons (bundled as separate runtime asset files; **not** embedded in the binary) | `assets/third_party/papirus/` | GPL-3.0 | `THIRD_PARTY_NOTICES/papirus/README.md` (full licence review), `assets/third_party/papirus/NOTICE.md` |
| Lucide icons (embedded in the binary via `icon_system.rs`) | `assets/icons/lucide/` | ISC | — |

These assets are separate works distributed alongside Boru. The GPL-3.0
Papirus icons are shipped as unmodified SVG data files loaded at runtime;
they are **not** compiled into the Boru executable, so the GPL does not
extend to Boru's own MIT/Apache-2.0 code (see the aggregate-work analysis in
`THIRD_PARTY_NOTICES/papirus/README.md`).

## 5. Redistributed native binaries (packaging-time only)

| Component | Location | Licence | Notice |
|---|---|---|---|
| GStreamer runtime + FFmpeg/GLib DLLs (Windows packaging only) | staged by `scripts/package_windows.sh` | LGPL-2.1-or-later (core/plugins/FFmpeg/GLib), BSD-3-Clause (vorbis/opus), Zlib, BSD-2-Clause (orc), bzip2 licence | `assets/third_party/gstreamer-notices/NOTICE.md`, `THIRD_PARTY_NOTICES/gstreamer/README.md` |

Notes:

- **Not committed to the repository.** No `.dll`/`.so`/`.dylib` is checked
  in. GStreamer/FFmpeg binaries are only staged into a Windows release
  package at packaging time, and the exact licence texts must be copied into
  `assets/third_party/gstreamer-notices/` as part of that release build
  (see `docs/video-runtime-packaging.md`).
- **OpenH264** is used for H.264 encoding (`openh264-sys2`, BSD-2-Clause) but
  the repo redistributes **no** OpenH264 binary: `openh264-sys2` compiles the
  upstream OpenH264 source into the Boru binary at build time.
- These native binaries are separate works under their own (LGPL/BSD/etc.)
  obligations and are **not** part of Boru's MIT/Apache-2.0 source licensing.

## 6. Licence gate

Boru's source is MIT OR Apache-2.0, and the compiled dependency graph must
stay permissively licensed. This is enforced automatically:

- **Config:** `deny.toml` — the `[licenses].allow` list **is** the gate.
  cargo-deny (>= 0.18.4) denies every licence that is not explicitly listed,
  and treats GNU licences pedantically (exact SPDX expression required).
- **Local command:**
  `./scripts/check-licenses.sh` (runs
  `cargo deny check licenses --workspace --all-features`); full check:
  `./scripts/check-licenses.sh --all`.
- **CI:** `.github/workflows/ci.yaml` — the `cargo_deny` job runs
  `cargo deny --workspace --all-features check -Dwarnings` on every PR/push.

A new GPL/AGPL (or any other copyleft) dependency therefore **fails CI**.
The only way to add one is an explicit review: list it under
`[[licenses.exceptions]]` in `deny.toml` with a justification comment
(`reason`), and never add copyleft licences to the `allow` list itself.

### Verification record (2026-08-14, DEBSRV, cargo-deny 0.20.2)

Gate passes on the current tree:

```
$ cargo deny --workspace --all-features check licenses
licenses ok        (exit 0)
```

Gate rejects a GPL dependency (scratch branch adding a crate declared
`GPL-3.0-only`; dependency removed afterwards — no GPL dep in any commit):

```
error[rejected]: failed to satisfy license requirements
  ┌─ scratch-gpltest-crate/Cargo.toml:7:12
  │
7 │ license = "GPL-3.0-only"
  │            ━━━━━━━━━━━━
  │            rejected: license is not explicitly allowed
  │
  ├ GPL-3.0-only - GNU General Public License v3.0 only:
  ├   - OSI approved
  ├   - FSF Free/Libre
  ├   - Copyleft
  ├ gpl-test-crate v0.1.0
    └── boru-core v0.201.0

licenses FAILED    (exit 4)
```

Note: the CI `cargo_deny` job's `check` also runs the advisory check. As of
2026-08-14 that separately reports several new rustsec advisories
(RUSTSEC-2023-0089, RUSTSEC-2024-0370, RUSTSEC-2026-0150, RUSTSEC-2026-0173,
RUSTSEC-2026-0207, RUSTSEC-2026-0208, RUSTSEC-2026-0212) in existing
dependencies — these are pre-existing findings independent of the licence
gate and are triaged separately.
