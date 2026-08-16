//! Emoji subsystem for the Boru desktop GUI (BORU-TWEMOJI-04).
//!
//! # Location decision
//!
//! The PDF plan calls for `src/ui/emoji/`, but Boru's GUI crate lives under
//! `examples/iced_chat/` (the `[[bin]] boru` target, `examples/iced_chat/main.rs`)
//! while `src/` is the headless `boru-core` library. Emoji artwork rendering is a
//! presentation-layer concern, so the module lives with the GUI crate at
//! `examples/iced_chat/emoji/`. The module boundary is the important part: all
//! Twemoji metadata, Unicode→asset resolution and SVG path knowledge is isolated
//! here and never leaks into chat/network code.
//!
//! # Module map
//!
//! - [`catalog`]       — emoji metadata and categories (catalog model: BORU-TWEMOJI-05)
//! - [`asset_manifest`] — generated index of vendored Twemoji asset keys and
//!                   the lookup surface (BORU-TWEMOJI-06)
//! - [`parser`]       — Unicode grapheme/emoji detection and asset-key
//!                   resolution (resolver: BORU-TWEMOJI-07; grapheme
//!                   segmentation of message text: BORU-TWEMOJI-16)
//! - [`renderer`]     — SVG handles, caching and the rendering abstraction
//!                   (full renderer/cache: BORU-TWEMOJI-08/09)
//! - [`emoji_text`]   — mixed text + Twemoji message renderer
//!                   (BORU-TWEMOJI-17; inline wrapping + baseline
//!                   hardening: BORU-TWEMOJI-18)
//! - [`picker`]       — the emoji picker panel (visual swap to SVG: BORU-TWEMOJI-10)
//!
//! # Small stable interfaces
//!
//! The rest of Boru consumes emoji through these module paths. Nothing
//! outside this module knows a Twemoji SVG filename or asset key.
//!
//! ```text
//! emoji::catalog::{Emoji, EmojiCategory, common_emojis}
//! emoji::asset_manifest::{contains, lookup, TWEMOJI_ASSETS}
//! emoji::parser::{MessageFragment, emoji_asset, split_fragments}
//! emoji::renderer::{cached_svg_handle, EmojiAsset, EmojiAssetCache, EmojiRenderer, TwemojiRenderer}
//!   — `EmojiRenderer::artwork` is the single shared fallback decision
//!   (SVG when resolved+loaded, original Unicode text otherwise;
//!   BORU-TWEMOJI-20), used by the picker and available to any surface.
//! emoji::emoji_text::{emoji_text, EmojiTextArtwork, EmojiTextStyle, plan_emoji_text, EMOJI_TEXT_SCALE}
//! emoji::picker::view_emoji_picker
//! ```
//!
//! Only [`EmojiCategory`] is additionally re-exported at `emoji::` (the
//! `AppMessage` enum and the chat panel reference it); every other item is
//! reached through the module paths above.
//!
//! # Dead-code note
//!
//! This is a bin crate (`[[bin]] boru`), so `pub` items that no code path
//! touches still trigger `dead_code`/`unused_imports`. BORU-TWEMOJI-24 removed
//! the chain-scaffolding blanket `#![allow(dead_code, unused_imports)]` and
//! the unused `pub use` re-exports; production consumers reach items through
//! the module paths above, and the remaining test-only helpers
//! (`asset_manifest::contains`, `catalog::{common_emojis, all_emoji,
//! REPRESENTATIVE_EMOJIS}`, `renderer::EmojiAssetCache::{len, is_empty}`) are
//! gated `#[cfg(test)]`. If a future task adds a new emoji surface, extend the
//! narrow re-export here instead of re-adding `#![allow(...)]`.

pub mod asset_manifest;
pub mod catalog;
pub mod emoji_text;
pub mod parser;
pub mod picker;
pub mod recents;
pub mod renderer;

// Narrow re-exports consumed outside the module. Consumers reach the other
// items through their module paths (`crate::emoji::picker::view_emoji_picker`,
// `crate::emoji::renderer::TwemojiRenderer`, `crate::emoji::recents::*`, …) —
// see the module map above.
pub use catalog::EmojiCategory;
