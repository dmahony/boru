# Boru Chat — Design Tokens

> **Superseded.** This file was the original chat-token specification from the
> pre-modern-redesign era and contains **stale values** that do not match the
> current implementation.

The authoritative, living specification for Boru visual tokens is
[`DESIGN_SYSTEM.md`](../DESIGN_SYSTEM.md), verified against the code:

- Token module: `examples/iced_chat/design_tokens.rs`
- Fonts/typography: `examples/iced_chat/fonts.rs`
- Icons: `examples/iced_chat/icon_system.rs`
- Shared components: `examples/iced_chat/ui_components.rs`
- Responsive breakpoints: `DESIGN_SYSTEM.md` §19.5

Do not rely on the palette values that used to live in this file (e.g.
`APP_BACKGROUND #F4F6F4`, `PRIMARY #2F6B4F`, `SIDEBAR_WIDTH 300`) — the modern
redesign moved to the Boru Modern spec values (`color_canvas #F7F9F8`,
`primary #188C50`, `SIDEBAR_WIDTH 304` with a 288–320 px responsive clamp).
