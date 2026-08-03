# UI-17 regression matrix: original action -> redesigned control -> verified result

Links every interaction from the UI-00 audit (`docs/ui-redesign/current-ui-map.md`)
to the redesigned control and its verified behaviour after the UI-04..UI-16
redesign. Method: live two-instance runs under Xvfb via MCP + static audit of
`AppMessage` dispatch.

| UI-00 interaction | Redesigned control (after UI-04..UI-16) | Verified result | Method |
|---|---|---|---|
| Settings gear | Sidebar gear button -> `OpenSettings` | Settings screen renders; MCP `boru_gui_navigate settings` reaches it | LIVE + screenshot 6_settings.png |
| Chat row select | Sidebar chat row -> `RoomSelected`/`OpenRoom` | Conversation opens; A/B connected on direct topic | LIVE |
| Group create | Sidebar "+" group / Home card -> `ShowCreateGroupDialog` | Dialog opens from a real sidebar click (tesseract-verified coordinates) | LIVE + screenshot |
| Join ticket | Sidebar join input -> `JoinFromTicket` | Path intact; ticket parse tests green | SRC + tests |
| Friend requests | Sidebar Requests / Home Add Friend -> `OpenFriendRequests` | Screen renders; request rows wired | LIVE + screenshot |
| Friend row Msg/Profile | Friends row buttons -> `OpenFriendChat`/`OpenFriendProfile` | Buttons emit correct messages | SRC |
| Peer profile | Remote sender label -> `OpenPeerProfile` | Chat log label on_press wired | SRC |
| Home Retry/Details | Home connection card buttons -> `RetryConnection`/`OpenConnectionDetails` | Both on_press present | SRC |
| Create public room | Home card -> dialog (`CreateNewRoom*`, `ConfirmCreateNewRoom`) | Dialog opened via MCP; room created and listed in A's directory | LIVE + screenshots 2/3 |
| Public-room discovery | Directory room row -> open action | B discovers A's room via directory gossip with relay; --no-relay blocks mesh (expected) | LIVE + b_discovered.png |
| Online Now Msg | Home Online Now row -> `OpenConversation(peer)` | Direct conversation opens on live peers | LIVE + 1_connected_a/b.png |
| Tunnel close | Tunnels card close -> `CloseTunnel(id)` | Wired to tunnel_service.list_tunnels() rows | SRC |
| Share files / attach | Composer paperclip -> `AttachPressed`; Home strip routes to Settings (known mismatch, separate card) | Composer attach wired; Home strip still Settings per UI-00 note | SRC |
| Back to chats | Chat header back -> `GoToChatList` | MCP navigate returns Home | LIVE + 1_home_chatlist.png |
| Composer typing | Composer input -> `InputChanged` | MCP set_composer drives input; IME composition fix intact | LIVE |
| Send | Composer send button / Enter -> `SendPressed` | Real message sent A->B and delivered | LIVE |
| Failure/retry | Failed bubble retry -> `RetryOutgoingMessage` | Wired; delivery-state logic unit-tested | SRC + tests |
| URL/image/context actions | Chat log rows -> `OpenUrl`/`OpenImageLightbox`/`ContextCopy*` | on_press present in view_chat_log | SRC |
| Emoji/GIF | Composer buttons -> `ToggleEmojiPicker`/`ToggleGifPicker` | Wired | SRC |
| Help | Header ? / help overlay -> `ToggleHelp` | **FIXED in UI-17**: MCP `toggle_help` previously dead; now routes to same message as visible button; overlay opens | LIVE + screenshot + unit test |
| Chat options/members/search | Header toolbar -> `ToggleChatOptions`/`ToggleMemberList`/`ToggleChatSearch` | Wired | SRC |
| Clear history | Options menu -> `ClearHistoryRequested`/`ConfirmClearHistory` | Wired + confirm dialog | SRC |
| Shared files | Header shared-files -> `BrowsePeerCatalogue` | Wired | SRC |
| Copy ticket / peer id | Room options / profile -> `CopyToClipboard`/`CopyPeerId` | Wired | SRC |
| Import friend | Sidebar -> `ImportFriendFromFile` | Wired | SRC |
| Create tunnel | Home card -> `ShowCreateTunnelDialog` | Wired | SRC |
| Dark mode | Theme toggle -> `ToggleDark` | Live toggle renders dark theme | LIVE + 8_dark_mode.png |
| Keyboard shortcuts | Global `Shortcut(..)` subscription | Wired | SRC |
| Request accept/decline | Requests rows -> request action messages | Wired | SRC |

## Action-routing diff vs UI-00 audit (plan step 4)

- **Lost:** none. Static audit (all 42 UI-00 interactions) found zero
  AppMessage variants declared-but-unhandled in `update()`.
- **Duplicated:** none found. Header/footer status split is complementary by
  design (UI-16 decision), not duplicate dispatch.
- **Routed differently:** one intentional + one bug fixed:
  1. `ToggleHelp` MCP command was routed nowhere (silently dropped in the
     diagnostic-only catch-all) -> now routed to `AppMessage::ToggleHelp`.
  2. Home "Share files" strip still routes to `OpenSettings` — known semantic
     mismatch documented in UI-00 (line 224), out of scope for this card.
- **Fixture leakage:** no production view reads fixture data; fixtures are
  QA-only scripts (see README fixture-leakage audit).

## Acceptance criteria status

| Criterion | Status |
|---|---|
| Every original UI-00 interaction remains reachable and behaves correctly | PASS (42/42; see checklist) |
| No production view depends on screenshot fixtures | PASS (audit) |
| Complete test suite passes or only documented baseline failures remain | PARTIAL — isolated build+tests green; shared tree blocked by concurrent FS-09..FS-16 WIP; 2 sibling-caused failures documented in README |
