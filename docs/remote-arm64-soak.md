# ARM64 remote soak workflow

The ARM64 Linux build uses the native glibc target shared by dragon, Orange Pi,
and Raspberry Pi hosts. It deliberately disables default wgpu/video features;
`gui,terminal` uses Boru's CPU-rendered GUI path and still exposes MCP.

## Build

From the repository root:

```sh
cargo build --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features \
  --features gui,terminal \
  --bin boru
```

The linker is configured in `.cargo/config.toml`. Verify the artifact before
deployment:

```sh
file target/aarch64-unknown-linux-gnu/release/boru
sha256sum target/aarch64-unknown-linux-gnu/release/boru
```

## Host prerequisites

Each remote host must have:

- `xvfb-run`
- `xauth`
- X11 runtime libraries used by winit/Iced (`libxcursor1`, `libxi6`,
  `libxkbcommon-x11-0`, and the normal X11/XCB dependencies)
- `python3` for remote MCP burst actions
- `sha256sum`
- SSH public-key access for the operator
- an ARM64 Linux userspace compatible with the glibc build

The remote controller checks architecture and these commands before deploying.
It does not install packages or change remote system configuration. Install
`xvfb` through the host's normal package manager if a check reports it missing.

## Remote orchestration

Copy `config/remote-soak-arm64.example.json` to a local operator manifest and
adjust only non-secret host/path/port values. Then run:

```sh
python3 scripts/remote_soak.py check \
  --manifest config/remote-soak-arm64.example.json \
  --run-dir /var/tmp/unused-check

python3 scripts/remote_soak.py deploy \
  --manifest config/remote-soak-arm64.example.json \
  --run-dir /var/tmp/unused-deploy

python3 scripts/remote_soak.py run \
  --manifest config/remote-soak-arm64.example.json \
  --duration-s 7200 \
  --interval-s 30 \
  --fault-every 4 \
  --run-dir /var/tmp/boru-soak/arm64-developer-$(date +%Y%m%d-%H%M%S)
```

The controller starts one process per SSH host under a dedicated Xvfb display,
uses a unique data directory and MCP port, records remote process metrics, and
performs restart/offline/burst fault actions. It stops each process group at
teardown and writes `report.json` plus `events.jsonl` locally. Large remote
logs remain under each node's configured data directory.

For fixture-backed runs, set a unique non-zero `bind_port` for each node in the
manifest. This lets the fixture runner construct explicit `EndpointAddr`
bootstrap entries when DHT/mDNS discovery is disabled.

## Fixture-backed application checks

When the nodes are running, use the companion fixture runner:

```sh
python3 scripts/rc_fixture.py \
  --manifest config/remote-soak-arm64.example.json \
  --bootstrap-room \
  --output /var/tmp/boru-rc-fixture/report.json
```

`--bootstrap-room` invokes the loopback-only `boru_gui_bootstrap_room` MCP
action with explicit peer IDs, addresses, and relay URLs, then uses the normal
room subscription path. The runner creates a deterministic remote source file,
registers it through `boru_gui_test_share_file`, fetches the recipient's signed
catalogue, downloads it through `boru_download_file`, and verifies size and
SHA-256 on the destination. It fails closed unless room membership, message
delivery, and file transfer all pass. Calls and screen sharing remain
`NOT_VALIDATED` and out of scope.

## Evidence limits

A successful controller report proves remote process lifecycle, architecture,
artifact hashes, fault actions, metrics collection, and cleanup. It does not
prove room convergence, transfer recovery, call lifecycle, or screen-share
assertions without a prepared MCP room fixture and fixture-specific actions.
Those assertions must remain explicitly `NOT_VALIDATED` until the fixture is
available.
