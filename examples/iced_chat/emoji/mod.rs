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
//! - [`catalog`]   — emoji metadata and categories (full model: BORU-TWEMOJI-05)
//! - [`parser`]    — Unicode grapheme/emoji detection and asset-key resolution
//!                   (full resolver: BORU-TWEMOJI-07)
//! - [`renderer`]  — SVG handles, caching and the rendering abstraction
//!                   (full renderer/cache: BORU-TWEMOJI-08/09)
//! - [`picker`]    — the emoji picker panel (visual swap to SVG: BORU-TWEMOJI-10)
//!
//! # Small stable interfaces
//!
//! The rest of Boru consumes emoji through these narrow re-exports. Nothing
//! outside this module knows a Twemoji SVG filename or asset key.
//!
//! ```text
//! emoji::catalog::{Emoji, EmojiCategory, common_emojis}
//! emoji::parser::{MessageFragment, emoji_asset, split_fragments}
//! emoji::renderer::{EmojiAsset, EmojiRenderer, TwemojiRenderer}
//! emoji::picker::view_emoji_picker
//! ```
//!
//! # Dead-code note
//!
//! This is a bin crate (`[[bin]] boru`), so `pub` items that no code path
//! touches yet still trigger `dead_code`/`unused_imports`. The re-exports
//! below are the stable interface later BORU-TWEMOJI tasks consume (catalog
//! model T5, manifest T6, resolver T7, renderer/cache T8/09, categories T12,
//! search T13, message rendering T16/17), so they are intentionally allowed.

#![allow(dead_code, unused_imports)]

pub mod catalog;
pub mod parser;
pub mod picker;
pub mod renderer;

pub use catalog::{common_emojis, Emoji, EmojiCategory};
pub use parser::{emoji_asset, split_fragments, MessageFragment};
pub use renderer::{EmojiAsset, EmojiRenderer, TwemojiRenderer};
