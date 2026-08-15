# Visual Designer Component IDs

The visual designer uses `examples/iced_chat/designer.rs::ComponentId` as the
single source of truth for editable component identity. IDs are semantic strings
and are stable across application runs, layout changes, and widget allocation.
They are suitable for TOML keys and inspector selections.

Do not derive an ID from a widget address, child order, or a temporary runtime
index. If a component is renamed, preserve the existing ID and update its label
or documentation separately.

| ID | Screen / section | Controls | Render site |
| --- | --- | --- | --- |
| `home.welcome` | Home dashboard welcome/header | Greeting, connection headline, and welcome status presentation | `examples/iced_chat/app/home.rs::IcedChat::view_chat_list_content` |
| `home.quick_actions` | Home dashboard quick actions | New-room, group, friend-request, and attach action cards | `examples/iced_chat/app/home.rs::IcedChat::view_chat_list_content` |
| `home.public_rooms` | Home dashboard public-room section | Public-room discovery/list entry point | `examples/iced_chat/app/home.rs::IcedChat::view_chat_list_content` |
| `home.friends` | Home dashboard friends/people section | Online friends and people activity content | `examples/iced_chat/app/home.rs::IcedChat::view_chat_list_content` |
| `home.recent_activity` | Home dashboard recent activity | Recent activity rows and empty state | `examples/iced_chat/app/home.rs::IcedChat::view_chat_list_content` |
| `sidebar` | Persistent navigation sidebar | Brand/identity header, navigation sections, and utility actions | `examples/iced_chat/app/sidebar.rs::IcedChat::view_sidebar` |
| `chat.message_list` | Active chat timeline | Scrollable message history, attachments, and delivery state | `examples/iced_chat/app/chat.rs::IcedChat::view_chat_log` |
| `chat.composer` | Active chat composer | Message input, send action, and composer affordances | `examples/iced_chat/app/chat.rs::IcedChat::view_composer` |

The Home IDs are registered together at the Home renderer boundary because the
Home dashboard is built by a cached static renderer. This keeps identity
independent of the configured section order and responsive column arrangement.
The Sidebar and Chat IDs are anchored directly at their render methods. All
anchors are compiled only with `dev-ui`; production rendering and behavior are
unchanged when the gate is disabled.
