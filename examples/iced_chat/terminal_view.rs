//! Embedded terminal tab, gated behind the `terminal` feature.
//!
//! Wraps [`iced_term::Terminal`] (which is powered by the
//! `alacritty_terminal` backend) so the chat GUI can host a real shell in a
//! tab. The feature flag `terminal` must be enabled for this module to be
//! compiled at all — see `main.rs` for the `#[cfg]`-gated module declaration.

use iced::Element;
use iced_term::{Terminal, TerminalView};

/// Wrapper around [`iced_term::Terminal`] that spawns the user's `$SHELL`
/// (falling back to `/bin/sh` when the variable is unset).
///
/// Exposes the four operations the app needs: construction, a view, event
/// handling, and a subscription. All real work is delegated to the
/// `iced_term` crate.
pub struct TerminalTab {
    /// The underlying iced_term terminal (id 0 — a single embedded tab).
    pub term: Terminal,
}

impl TerminalTab {
    /// Spawn a terminal running `$SHELL` (fallback `/bin/sh`).
    ///
    /// Mirrors the `iced_term` `full_screen` example: `Terminal::new`
    /// immediately creates the PTY and starts the shell's event loop, so the
    /// shell process exists even before the tab is first shown.
    pub fn new() -> std::io::Result<Self> {
        let system_shell =
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let settings = iced_term::settings::Settings {
            font: iced_term::settings::FontSettings::default(),
            theme: iced_term::settings::ThemeSettings::default(),
            backend: iced_term::settings::BackendSettings {
                program: system_shell,
                ..Default::default()
            },
        };
        Ok(Self {
            term: Terminal::new(0, settings)?,
        })
    }

    /// Render the terminal widget. Emits [`iced_term::Event`]s (e.g. resize,
    /// key, mouse) that the caller maps into its own message type.
    pub fn view(&self) -> Element<'_, iced_term::Event> {
        TerminalView::show(&self.term)
    }

    /// Forward a backend command produced by the view or the subscription
    /// stream into the terminal. Returns the resulting action so the caller
    /// can react (e.g. shell exited → `Shutdown`).
    pub fn update(
        &mut self,
        cmd: iced_term::BackendCommand,
    ) -> iced_term::actions::Action {
        self.term.handle(iced_term::Command::ProxyToBackend(cmd))
    }

    /// Stream of terminal backend events (PTY output, exit, …).
    pub fn subscription(&self) -> iced::Subscription<iced_term::Event> {
        self.term.subscription()
    }
}
