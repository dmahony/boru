# UI-17 interaction checklist (pass/fail per original UI-00 interaction)

Method: two live instances (A/B) over direct QUIC, driven through the loopback
MCP (`--mcp --enable-gui-test-actions`) with `scripts/ui_mcp.py`, plus static
audit of `AppMessage` -> `update` coverage. PASS = observed live or verified by
unit test + source mapping; FAIL = broken; N/A = not applicable at this
screen/state; SRC = verified by source mapping/static audit only.

Legend: LIVE = exercised on real instances; SRC = source-mapped; TEST = unit
test.

| # | UI-00 interaction | AppMessage | Result | Evidence |
|---|---|---|---|---|
| 1 | Settings gear | `OpenSettings` | PASS (LIVE) | 6_settings.png; MCP navigate |
| 2 | Collapse sidebar sections | `ToggleSidebarSectionCollapsed` | PASS (SRC) | sidebar click path + update arm |
| 3 | Chat row select | `RoomSelected` / `OpenRoom` | PASS (LIVE) | conversation opened on A and B |
| 4 | Group create | `ShowCreateGroupDialog` | PASS (LIVE) | t_8de1c9c4_group_dialog_1024x768.png |
| 5 | Group row | `OpenGroupChat` | PASS (SRC) | update arm + sidebar group rows |
| 6 | Join ticket input | `JoinTicketInputChanged` | PASS (SRC) | composer/join input path |
| 7 | Join ticket submit | `JoinFromTicket` | PASS (SRC) | update arm + ticket parser tests |
| 8 | Friend requests screen | `OpenFriendRequests` | PASS (LIVE) | 5_friend_requests.png |
| 9 | Friend row Msg | `OpenFriendChat` | PASS (SRC) | update arm + friends rows |
| 10 | Friend profile | `OpenFriendProfile` | PASS (SRC) | update arm + friends rows |
| 11 | Peer profile | `OpenPeerProfile` | PASS (SRC) | update arm + remote sender label |
| 12 | Home Retry | `RetryConnection` | PASS (SRC) | home connection card button |
| 13 | Home Details | `OpenConnectionDetails` | PASS (SRC) | home connection card button |
| 14 | Create public room | `CreateNewRoom` (+ dialog msgs) | PASS (LIVE) | 2_create_room_dialog.png, 3_room_created.png |
| 15 | Online Now Msg | `OpenConversation(peer)` | PASS (LIVE) | 1_connected_a.png / b |
| 16 | Tunnel close | `CloseTunnel` | PASS (SRC) | tunnels card + update arm |
| 17 | Share files / attach | `AttachPressed` | PASS (SRC) | composer attach + update arm |
| 18 | Back to chats | `GoToChatList` | PASS (LIVE) | MCP navigate + 1_home_chatlist.png |
| 19 | Composer typing | `InputChanged` | PASS (LIVE) | composer set via MCP |
| 20 | Send | `SendPressed` | PASS (LIVE) | real message A->B delivered |
| 21 | URL click | `OpenUrl` | PASS (SRC) | chat log link on_press |
| 22 | Image click | `OpenImageLightbox` | PASS (SRC) | chat log image on_press |
| 23 | Close image | `CloseImageLightbox` | PASS (SRC) | lightbox close |
| 24 | Retry failed | `RetryOutgoingMessage` | PASS (SRC) | failed bubble retry + tests |
| 25 | Context copy text | `ContextCopyText` | PASS (SRC) | context menu on_press |
| 26 | Context copy image | `ContextCopyImage` | PASS (SRC) | context menu on_press |
| 27 | Close context menu | `CloseContextMenu` | PASS (SRC) | context menu close |
| 28 | GIF picker toggle | `ToggleGifPicker` | PASS (SRC) | composer GIF button |
| 29 | Emoji picker toggle | `ToggleEmojiPicker` | PASS (SRC) | composer emoji button |
| 30 | Help toggle | `ToggleHelp` | PASS (LIVE + TEST) | 9_help_overlay.png, t_8de1c9c4_help_overlay_1280x800.png, gui_help_command_maps_to_normal_toggle_message |
| 31 | Chat options | `ToggleChatOptions` | PASS (SRC) | header options button |
| 32 | Member list | `ToggleMemberList` | PASS (SRC) | header members button |
| 33 | Chat search | `ToggleChatSearch` | PASS (SRC) | header search button |
| 34 | Clear history | `ClearHistoryRequested` / `ConfirmClearHistory` | PASS (SRC) | options menu + confirm |
| 35 | Copy peer id | `CopyPeerId` | PASS (SRC) | profile/context path |
| 36 | Shared files | `BrowsePeerCatalogue` | PASS (SRC) | header shared-files button |
| 37 | Copy ticket | `CopyToClipboard` | PASS (SRC) | room options ticket row |
| 38 | Import friend | `ImportFriendFromFile` | PASS (SRC) | sidebar import path |
| 39 | Create tunnel | `ShowCreateTunnelDialog` | PASS (SRC) | home action card |
| 40 | Dark mode | `ToggleDark` | PASS (LIVE) | 8_dark_mode.png |
| 41 | Keyboard shortcuts | `Shortcut(..)` | PASS (SRC) | subscription/shortcut path |
| 42 | Friend request accept/decline | request action msgs | PASS (SRC) | requests rows + update arm |

## Integration scenarios (plan step 2)

| Scenario | Result | Notes |
|---|---|---|
| Connection startup (A+B boot, friend online) | PASS (LIVE) | 1_connected_a.png / b |
| Peer offline (kill B) | PARTIAL | offline state exercised via SetPeerPresence production path; live friend-ping cadence > 75 s so capture was partial (t_8de1c9c4_peer_offline_1280x800.png) |
| Reconnect (restart B) | PASS (LIVE) | 3_reconnected_a.png |
| Friend changes | PASS (SRC) | FriendEvent StatusChanged path used by SetPeerPresence |
| Group changes | PASS (LIVE) | group dialog via real sidebar click |
| Public-room discovery | PASS (LIVE) | b_discovered.png (relay run); --no-relay blocks directory mesh (expected) |
| Requests | PASS (LIVE) | 5_friend_requests.png |
| Tunnel changes | PASS (SRC) | tunnels card renders tunnel_service.list_tunnels(); CloseTunnel wired |
| File-sharing entry points | PASS (LIVE) | 7_file_sharing.png; AttachPressed wired |

## Chat scenarios (plan step 3)

| Scenario | Result | Notes |
|---|---|---|
| History load | PASS (LIVE) | 4_history_restored_a.png (entries restored after restart) |
| Send | PASS (LIVE) | composer submit via MCP; message delivered A->B |
| Receive | PASS (LIVE) | B timeline shows A message |
| Failure/retry | PASS (SRC) | RetryOutgoingMessage wired; join-request retry test exists |
| Delivery/read updates | PASS (SRC) | presentation.rs delivery labels; DeliveryState enum round-trip tested |
| Rename/join events | PASS (SRC) | system entries + update arms |
| Toolbar actions | PASS (LIVE) | back/search/details/options navigation verified |
| Composer actions | PASS (LIVE) | set/submit/clear via MCP; attach path wired |

## Regression matrix

See `regression-matrix.md` for action -> redesigned control -> verified result.
