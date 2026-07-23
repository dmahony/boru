# Notification System Implementation — Status

## Files Created/Modified

### New files (`examples/iced_chat/notification/`)

| File | Line Count | Purpose |
|------|-----------|---------|
| `mod.rs` | ~50 | Module root with architecture docs |
| `event.rs` | ~450 | NotificationEvent, NotificationEventKind, priorities, actions, action targets |
| `service.rs` | ~650 | NotificationService — policy checks, dedup, rate limit, grouping |
| `backend.rs` | ~350 | NotificationBackend trait, DevBackend, NoopBackend (with tests) |
| `focus.rs` | ~270 | WindowFocusTracker — focus, visibility, minimised state |
| `render.rs` | ~280 | PreviewMode, render_event, sanitize_preview, group summaries (with tests) |

### Modified files

| File | Change |
|------|--------|
| `examples/iced_chat/main.rs` | Added `mod notification;` declaration |

### New documentation (`docs/notification-system/`)

| File | Purpose |
|------|---------|
| `01-audit-and-plan.md` | Complete architecture audit and implementation plan |

## Step Coverage

| Step | Status | Where |
|------|--------|-------|
| 1: Audit existing event flow | **Done** | `docs/notification-system/01-audit-and-plan.md` |
| 2: Define Notification Event Types | **Done** | `notification/event.rs` |
| 3: Create Notification Service Interface | **Done** | `notification/service.rs`, `notification/backend.rs` |
| 4: Centralise Window And Focus State | **Done** | `notification/focus.rs` |
| 5: Improve Unread Message State | **Built** | Existing `app.rs` logic + `NotificationService` policy check |
| 6: Development Notification Backend | **Done** | `notification/backend.rs` — DevBackend |
| 7: Message Notification Rendering | **Done** | `notification/render.rs` — 3 privacy modes |
| 8: Message Deduplication | **Done** | `notification/service.rs` — DedupCache (60s TTL, 500 entries) |
| 9: Message Grouping | **Done** | `notification/service.rs` — GroupState (5s window, per-conversation) |
| 10: Native Desktop Backend | **Pending** | Needs `notify-rust` crate, see below |
| 11: Notification Clicks | **Pending** | Needs app.rs wiring |
| 12: Mark-As-Read Actions | **Pending** | Needs app.rs wiring |
| 13: Friend-Request Notifications | **Built** | Event kind exists, emission pending in app.rs |
| 14: File-Transfer Notifications | **Built** | Event kinds exist, emission pending in app.rs |
| 15: Connection Notifications | **Built** | Event kinds exist with priority/rate-limiting |
| 16: Notification Settings | **Pending** | UI integration |
| 17: Per-Conversation Mute | **Pending** | Data model needed in ConversationEntry |
| 18: Do Not Disturb | **Pending** | Schedule logic |
| 19: System-Tray Support | **Pending** | Iced tray API |
| 20: Minimise-To-Tray | **Pending** | Depends on tray |
| 21: Application Badges | **Pending** | Tray + taskbar badges |
| 22: Startup Recovery | **Pending** | Unread persistence |
| 23: Privacy/Security Hardening | **Done** | `notification/render.rs` — sanitize_preview |
| 24: Rate Limiting | **Done** | `notification/service.rs` — RateLimiter |
| 25: Diagnostics | **Pending** | Integration with existing diagnostics |
| 26: Expand Platform Support | **Future** | macOS, Windows |
| 27: Automated Testing | **In progress** | ~50 unit tests across all modules |
| 28: Manual Testing | **Future** | Needs GUI environment |
| 29: Documentation | **Started** | Module docs + architecture overview |
| 30: Final Review | **Future** | Before merge |

## Architecture Summary

```
App events (NetEvent, FriendEvent, WhisperEvent, TransferProgress)
    │
    ▼ (event emission in app.rs — pending wiring)
NotificationService::handle_event(event)
    │
    ├── 1. Master toggle check (enabled/disabled)
    ├── 2. Per-kind toggle check
    ├── 3. Focus check (focused + same conversation → suppress)
    ├── 4. Dedup cache (60s TTL, 500 entry LRU)
    ├── 5. Rate limiter (5 events/kind/10s, High priority bypasses)
    ├── 6. Render (PreviewMode: full/sender_only/hidden)
    └── 7. Group (5s window, by group_key/conversation)
            │
            ▼
        RenderedNotification
            │
            ▼
        NotificationBackend (DevBackend / NoopBackend / NativeBackend)
```

## Next Steps (in priority order)

1. **Add `notify-rust` dependency** to Cargo.toml and implement NativeBackend
2. **Wire `NotificationService` into `IcedChat`** in `app.rs`:
   - Instantiate service in `new()`
   - Emit `NotificationEvent` in the `AppMessage::NetEvent` handler
   - Emit events in `WhisperEvent` handler (friend requests)
   - Emit events in `on_transfer_progress()` (file transfers)
3. **Wire `WindowFocusTracker` into Iced subscriptions**:
   - Subscribe to `iced::window::focus_events()` and `iced::window::mode_events()`
   - Track `active_conversation_id` on screen changes
   - Forward focus state to `NotificationService`
4. **Add notification settings** to settings UI and `AppSettings` struct
5. **Add per-conversation mute** to `ConversationEntry`
6. **Add system tray** support
