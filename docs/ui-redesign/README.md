# UI redesign evidence and screenshot harness

## Baseline capture

Build the GUI binary, then run one command from the repository root:

```sh
cargo build --features gui --bin boru
scripts/ui_baseline_screenshots.sh
```

The script captures Home and Chat at 1280x800, 1024x720, and 1440x900 under:

```text
docs/ui-redesign/evidence/baseline/
```

The naming convention is:

```text
<t-task-id>_<screen>_<width>x<height>_<state>.png
```

For this baseline the task ID is `t_9ec8d24f`, and `state` is `baseline`.

## How the harness works

- Starts `target/debug/boru` with `open`, so a fresh temporary data directory creates a deterministic local room for the Chat screen.
- Uses `--name "UI Baseline"`, `--no-dht`, and `--no-relay`; no network identifiers or private keys are committed.
- Enables the existing loopback-only GUI test MCP (`127.0.0.1`) solely to navigate from Chat to Home.
- Starts a disposable Xvfb screen at the requested size and captures the Boru window with ImageMagick `import`.
- Redacts the Discover peer rows in the committed PNGs because mDNS may expose real local peer IDs; the rest of the sidebar remains unchanged.
- Removes the temporary data directory and processes on exit.
- Refuses to overwrite an existing baseline image.

This is a development/documentation script. It does not change release behavior and does not add fixture data to production paths. The only deterministic visible fixture is the local display name and freshly-created local room; network-backed cards remain in their real empty/offline state.

The MCP helper is `scripts/ui_mcp.py`; it uses only the Python standard library and sends requests to loopback.

## Visual review

Use `docs/ui-redesign/ui-visual-qa-checklist.md` for the shell, sidebar, header, cards, message timeline, composer, and footer regions at every viewport.
