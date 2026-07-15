GUI test report — task t_57b0139c

Target VM: local Ubuntu VM hostname 7070
Captured: 2026-07-15T01:15:29Z
Environment: Linux 6.8.0-134-generic x86_64; DISPLAY=:101; Xvfb; app commit abecabdb7d73eac78b892950a30c3921907e7177; app version v0.101.0 (97689333); node 4855442a70dd5aa978924880625b94ec8ab6af4616e8333d5c60bf60d81cf170.
MCP endpoint: 127.0.0.1:8765, responsive.

Scope attempted
- boru_get_gui_snapshot
- boru_gui_navigate(destination=chat_list)
- boru_gui_set_composer(text="MCP GUI diagnostic test")
- boru_gui_submit_composer
- boru_run_gui_message_test(room_id=9021bd1ed0932e4fb1dfd5477ebee17916eb431c892d44f16287962329eaf303, message_text="complete GUI workflow test", expected_peer_id=<local node>, timeout_ms=10000)
- boru_gui_get_action_status for all three run-test actions
- final node, room, discovery-event, and GUI snapshots
- X11 root screenshot captured as gui_failure_evidence.xwd (1280x1024x24, 5,246,059 bytes, SHA-256 1c0e57c6457423aa229f6be12b62136964980e1d70d25fab0123f0f80645a8c3)

Observed results
1. Initial GUI snapshot: gui_test_actions_enabled=true, journal_entry_count=0, diagnostics_event_count=0.
2. chat_list navigation accepted and action completed. GUI journal sequence advanced.
3. Composer update accepted and action completed; text length 23.
4. Composer submit accepted by MCP, but action status was rejected with error code send_disabled and message "Sending is disabled until the room is subscribed".
5. boru_run_gui_message_test returned after 10.011s:
   success=false
   first_failed_stage=local_application_state
   room_navigation=true
   composer_update=true
   composer_submission=true
   composer_cleared=false
   local_message_created=false
   local_gui_state=true
   final GUI state: active_room=9021bd1ed0932e4fb1dfd5477ebee17916eb4316e8333d5c60bf60d81cf170, active_screen=Chat, composer_text="complete GUI workflow test", total_entry_count=0, mesh_health="Offline(\"Not connected to any room\")", neighbor_count=0, direct_peer_count=0, dialog_open=false.
   Three GUI journal entries were observed (room navigation, composer update, composer submission), all message_variant GuiTestActionReceived and success=true.
6. Final room status: joined=true, subscribed=true, local_room_joined=false, peer_count=0, peers=[], last_error=null; discovery sources mdns/mainline_dht/bootstrap.
7. Final discovery events: returned_count=0, latest_sequence=0.

Failure isolation
The GUI action transport and GUI state observation work. The first failed application stage is local_application_state, specifically room subscription readiness as exposed by the rejected SubmitComposer action. The local message pipeline did not create a message (total_entry_count stayed 0), and no remote delivery was attempted/verified. Network/discovery failure is independently evidenced by zero peers and zero discovery events; remote VM MCP was unavailable per parent readiness handoff.

Reproduction
1. Start the local app with --mcp --enable-gui-test-actions --mcp-bind 127.0.0.1:8765 under Xvfb.
2. Call boru_gui_open_room or boru_gui_navigate(destination=chat_list).
3. Call boru_gui_set_composer with nonempty text, then boru_gui_submit_composer.
4. Query boru_gui_get_action_status using the returned submit action id.
5. Observe state=rejected, error.code=send_disabled, error.message="Sending is disabled until the room is subscribed"; query boru_get_room_status and observe local_room_joined=false/peer_count=0.

Overall: FAIL (GUI controls and action journal pass; message workflow fails at local room subscription/application readiness). Remote symmetric delivery: Not Observed because the second VM MCP server accepts TCP but does not reply.
