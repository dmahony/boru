# Friend request visibility repro

## Environment
- Repo: `/home/dan/iroh-gossip-chat`
- Repro workspace: `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_aaf9140d/repro`

## Steps to reproduce
1. Create two separate identities using distinct data directories.
2. Send a friend request from identity A to identity B.
3. Verify identity B receives an incoming pending request.
4. Accept the request on identity B and reload both stores.

## Repro command
Run from `/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_aaf9140d/repro`:

```bash
cargo run -q
```

## Logged output
```text
alice_pk=3feb9dc6be5e5e3af5dcd420bb9f20f28382d6aab6bec58f258a2e529f1cd875
bob_pk=953add0a19d694bc46a30fd6dfd744b6e49315aa80d648994babbef1b978c1e9
alice_dir=/tmp/boru-fr-repro-alice-1783948408891378822
bob_dir=/tmp/boru-fr-repro-bob-1783948408891396769
alice send_request id=3feb9dc6:953add0a:19f5b9c783b status=Pending
verified from=3feb9dc6be5e5e3af5dcd420bb9f20f28382d6aab6bec58f258a2e529f1cd875 action=FriendRequest { name: None }
bob send_request id=3feb9dc6:953add0a:19f5b9c7844 status=Pending
alice_outgoing_count=1
bob_incoming_count=1
alice_outgoing_first=Some(("3feb9dc6:953add0a:19f5b9c783b", "3feb9dc6be5e5e3af5dcd420bb9f20f28382d6aab6bec58f258a2e529f1cd875", "953add0a19d694bc46a30fd6dfd744b6e49315aa80d648994babbef1b978c1e9", Pending, Some("hi bob")))
bob_incoming_first=Some(("3feb9dc6:953add0a:19f5b9c7844", "3feb9dc6be5e5e3af5dcd420bb9f20f28382d6aab6bec58f258a2e529f1cd875", "953add0a19d694bc46a30fd6dfd744b6e49315aa80d648994babbef1b978c1e9", Pending, None))
bob_incoming_pending_after_accept=0
bob_incoming_accepted_after_accept=1
alice_loaded_len=1
bob_loaded_len=1
alice_loaded_status=[("3feb9dc6:953add0a:19f5b9c783b", Pending)]
bob_loaded_status=[("3feb9dc6:953add0a:19f5b9c7844", Accepted)]
```

## Conclusion
The request is returned to the recipient-side store: Bob sees 1 incoming pending request immediately after the send/receive flow. I did not hit any console errors or failed API calls in this repro.
