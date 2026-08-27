# BORU-NEXT-03 seeded-peer GUI smoke evidence

Date: 2026-08-27
Source commit: `4a57a5c7cd7679cb3b51aaeab107d77b97845ca5`
Build: `rb build --bin boru --features gui,video-playback`
Artifact SHA-256: `194c23989f62bdab579f106dbbac9f3dd213725b62286c2790d2de52319de9a1`

## Deployment

PASS — both Ubuntu GUI verification VMs received `/home/dan/boru-test/boru-x86_64-linux` with the exact artifact SHA-256 above.

PASS — both VMs launched the artifact with native X11 settings and `--enable-gui-test-actions`.

PASS — runtime process and MCP listener verified independently:
- vm-a: `127.0.0.1:9054`, visible Boru window found by `xdotool search --name Boru`
- vm-b: `127.0.0.1:9055`, visible Boru window found by `xdotool search --name Boru`

Runtime log evidence:
- `/home/dan/boru-test/runs/vm-a/instance.log`
- `/home/dan/boru-test/runs/vm-b/instance.log`
- `/home/dan/boru-test/runs/vm-a/logs/boru.log`
- `/home/dan/boru-test/runs/vm-b/logs/boru.log`

## Seeded-peer smoke

PASS — explicit seeded-peer bootstrap created and joined room `b8430ac687ba45796031d5f77d84ceda0c49d1afef92a2a4db31b250dba093bc` on both nodes.

PASS — GUI room navigation, composer update, and composer submission on both nodes. The harness recorded successful Iced update journal entries and a live chat screen with one mesh neighbor.

PASS — normal GUI message pipeline in both directions (vm-a and vm-b). The harness itself correctly notes this validates the local GUI pipeline; room membership and reciprocal seeded-peer state were also validated separately.

PASS — visible file-share/download flow supported by the fixture. A deterministic 4,120-byte fixture transferred vm-a → vm-b and remote SHA-256 matched `330d51f2ac8759c6fe4d33ef966fb9aba78f044d907f6a16d48796cb016d7208`.

NOT VALIDATED — conversation deletion persistence. No deletion action is exposed by the available seeded-peer fixture, so no pass is claimed.

NOT AVAILABLE — call and screen-share lifecycle; outside this task's available fixture.

Structured evidence: `artifacts/seeded-peer-smoke.json`
Manifest/procedure: `artifacts/seeded-peer-manifest.json`, `scripts/rc_fixture.py --manifest artifacts/seeded-peer-manifest.json --bootstrap-room ...`
