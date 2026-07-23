//! Bundled font loading for the Boru desktop app.
//!
//! Embeds Montserrat fonts (Black 900, ExtraBold 800, Bold 700) at
//! compile time via `include_bytes!` and loads them into the Iced font
//! system at app startup.
//!
//! ## Font IDs
//!
//! | Constant               | Family name           | Weight |
//! |------------------------|-----------------------|--------|
//! | `MONTSERRAT_BLACK`     | "Montserrat Black"    | 900    |
//! | `MONTSERRAT_EXTRA_BOLD` | "Montserrat ExtraBold"| 800    |
//! | `MONTSERRAT_BOLD`      | "Montserrat Bold"     | 700    |
//!
//! ## Licence
//!
//! Montserrat is licensed under the SIL Open Font License 1.1.
//! See fonts/OFL.txt for the full licence text.

use iced::font;

// ── Bundled font data ────────────────────────────────────────────────────────

/// Montserrat Black 900 — the primary branded wordmark weight.
const MONTSERRAT_BLACK_BYTES: &[u8] = include_bytes!("fonts/Montserrat-Black.ttf");

/// Montserrat ExtraBold 800 — for high-impact headings.
const MONTSERRAT_EXTRA_BOLD_BYTES: &[u8] =
    include_bytes!("fonts/Montserrat-ExtraBold.ttf");

/// Montserrat Bold 700 — for less forceful branded headings.
const MONTSERRAT_BOLD_BYTES: &[u8] = include_bytes!("fonts/Montserrat-Bold.ttf");

// ── Font family identifiers ──────────────────────────────────────────────────

/// Family name used with `iced::Font::with_name` to select the
/// Montserrat Black weight.  Must match the internal font name
/// (not the filename).
pub const MONTSERRAT_BLACK: &str = "Montserrat Black";

/// Family name for Montserrat ExtraBold 800.
pub const MONTSERRAT_EXTRA_BOLD: &str = "Montserrat ExtraBold";

/// Family name for Montserrat Bold 700.
pub const MONTSERRAT_BOLD: &str = "Montserrat Bold";

// ── Font loading ─────────────────────────────────────────────────────────────

/// Returns an `iced::Task` that loads all bundled Montserrat fonts into
/// the Iced runtime.  Call once at application startup, chained onto the
/// initial command returned by `Application::new`.
///
/// The returned task fires the given message tag on completion; the
/// loading result can be ignored (errors are non-fatal — the system falls
/// back to the default sans-serif font).
pub fn load_fonts() -> iced::Task<crate::app::AppMessage> {
    iced::Task::batch(vec![
        font::load(MONTSERRAT_BLACK_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(MONTSERRAT_EXTRA_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(MONTSERRAT_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
    ])
}
