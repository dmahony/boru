Boru Home PDF pp. 12-13 verification

Revision

Worktree HEAD: edab3e00 (HOME-PDF-05 integration commit 0ec98fe9 is
cherry-picked onto the task branch). The integrated change composes the detailed
mesh/status card with the compact mesh summary; it does not alter the protected
sidebar, hero, logo, or connection-card routes.

Automated evidence

Commands run from this worktree:

    git fetch origin && git merge origin/main
    rb check --bin boru --features gui,video-playback,terminal
    rb test --bin boru --features gui,video-playback,terminal -- home_

Results:

    rb check: PASS (remote DEBSRV build; existing warning set only)
    home_ filter: PASS, 46 passed, 0 failed
    git diff --check origin/main..HEAD: PASS

The 46-test run covers Home projections, online/offline/mixed presence, empty
and capped activity (including repeated IDs and distinct identical content),
tunnel counts, responsive tiers and content widths, natural wrapping, labels,
route mappings, card construction, status variants, and layout TOML defaults,
partial overrides, malformed values, bounds, semantic validation, rearrange,
and reload preservation. The individual tests are listed in the command output
and are reproducible with the command above.

Runtime visual evidence already tracked in this checkout

These are component/offscreen captures, not claims of a full desktop pass:

    captures/home_light.png (1200x800) — empty Home, light theme
    captures/home_dark.png (1200x800) — empty Home, dark theme
    captures/status_ready_w450.png (450x480) — narrow status card
    captures/status_ready_w679.png (679x360) — medium status card
    captures/status_ready_w1215.png (1215x360) — wide status card
    captures/create_tunnel_dialog_light.png (1200x800) — tunnel dialog shell

They are produced by the existing offscreen capture tests, including:

    rb test --bin boru --features gui,video-playback,terminal -- home_

and can be inspected with the PNG dimensions above. The captures demonstrate
empty-state rendering, light/dark runtime theme surfaces, narrow/medium/wide
status geometry, and the tunnel dialog shell. They do not prove live peer,
active tunnel, populated file, accessibility, or physical-DPI behavior.

Required visual matrix and honest limits

    baseline / 1200x800: AVAILABLE — tracked Home light and dark captures
    1920x1080: NOT CAPTURED in this task worktree
    approximately 1672x941: NOT CAPTURED in this task worktree
    1440x900: NOT CAPTURED in this task worktree
    1280x800: NOT CAPTURED in this task worktree
    smallest supported: PARTIAL — 450px status component capture; no full-app
        minimum-viewport capture
    100% scaling: logical component tests only
    125/150/200% scaling: NOT AVAILABLE under the headless/offscreen harness

A DEBSRV executable copy was not used for screenshots: after the successful
check and test runs, the required remote debug build failed with ENOSPC while
archiving iroh. No screenshot is represented as coming from that failed build.
The earlier failed filtered test invocation also ended with Cargo's broken-pipe
listing error; it was replaced by the successful single `home_` run above.

Scenario matrix

    Quick Actions: automated route and four-action coverage PASS; pointer/Enter
        interaction screenshot NOT RUN.
    People and Activity: online/away/offline filtering, count, capped rows,
        stable IDs and wrapping PASS in tests; live populated screenshot NOT RUN.
    Tunnels: live-connection count versus saved records and reconnect state PASS;
        active/pending/failure/>3-row live screenshot NOT RUN.
    Empty Home: light and dark component captures AVAILABLE.
    Long/narrow text and breakpoint-adjacent geometry: automated PASS; only
        component-width captures are available.
    old/bad/alternate-accent runtime theme: NOT RUN; no claim is made.
    keyboard focus, accessibility contrast, permissions, cancellation and
        actual network/media behavior: NOT RUN; compilation/tests do not imply
        those passes.

Protected-region comparison

No source changes were made to sidebar, hero/logo assets, or connection-card
routing. The only code delta in this task branch is the HOME-PDF-05 composition
fix in src/bin/boru/app/home.rs, plus this evidence record. Therefore protected
region geometry was source-reviewed but not declared visually passed at the
unavailable requested desktop sizes.

Reproduction notes

Use DEBSRV for cargo commands via rb. For a future desktop capture, build with
`rb build --bin boru --features gui,video-playback,terminal`, obtain the fresh
binary only after verifying its mtime is newer than HEAD, then run each logical
viewport under Xvfb/scrot with `--no-relay --no-dht --bind-port 0` and record
PNG dimensions with `file`. Seeded peers/tunnels and physical DPI require a
real desktop session; they are intentionally untested here.
