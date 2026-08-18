//! Embedded terminal tab, gated behind the `terminal` feature.
//!
//! Wraps [`iced_term::Terminal`] (which is powered by the
//! `alacritty_terminal` backend) so the chat GUI can host a real shell in a
//! tab. The feature flag `terminal` must be enabled for this module to be
//! compiled at all — see `main.rs` for the `#[cfg]`-gated module declaration.
//!
//! On startup the shell prints an ASCII-art MOTD banner (`motd.txt`, bundled
//! at compile time) before handing off to the user's interactive shell.

use iced::Element;
use iced_term::{Terminal, TerminalView};

/// ASCII-art MOTD banner shown when the embedded terminal opens.
///
/// Bundled into the binary with `include_str!` so it travels with the
/// executable — no file lookup on disk. Printed by the shell's `-c` startup
/// command (see [`TerminalTab::startup_command`]) before the interactive
/// shell takes over.
const MOTD: &str = include_str!("motd.txt");

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

/// Wrap `s` in single quotes, escaping embedded single quotes the POSIX way
/// (`'` → `'\''`), so it is safe to interpolate into a shell `-c` string.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl TerminalTab {
    /// Build the shell `-c` startup command: print the ASCII MOTD banner,
    /// then `exec` the user's shell so it starts interactively (reads rc
    /// files, shows a prompt) exactly as if it had been spawned directly.
    fn startup_command(shell: &str) -> String {
        // printf is POSIX and works in sh/bash/zsh/fish; %s never interprets
        // escapes inside the banner text itself.
        format!(
            "printf '%s\\n' {}; exec {}",
            shq(MOTD.trim_end()),
            shq(shell)
        )
    }

    /// Spawn a terminal running the platform shell.
    ///
    /// On Unix this runs `$SHELL` (fallback `/bin/sh`) with `-c` so the MOTD
    /// banner prints before the interactive shell takes over. On Windows the
    /// POSIX `printf`/`exec` startup command does not exist, so we spawn the
    /// user's console (`%COMSPEC%`, fallback `cmd.exe`) interactively and
    /// skip the banner.
    ///
    /// Mirrors the `iced_term` `full_screen` example: `Terminal::new`
    /// immediately creates the PTY and starts the shell's event loop, so the
    /// shell process exists even before the tab is first shown.
    pub fn new() -> std::io::Result<Self> {
        #[cfg(windows)]
        let (program, args): (String, Vec<String>) = {
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
            (comspec, Vec::new())
        };
        #[cfg(not(windows))]
        let (program, args): (String, Vec<String>) = {
            let system_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            (
                system_shell.clone(),
                vec!["-c".to_string(), Self::startup_command(&system_shell)],
            )
        };
        let settings = iced_term::settings::Settings {
            font: iced_term::settings::FontSettings::default(),
            theme: iced_term::settings::ThemeSettings::default(),
            backend: iced_term::settings::BackendSettings {
                program,
                args,
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
    pub fn update(&mut self, cmd: iced_term::BackendCommand) -> iced_term::actions::Action {
        self.term.handle(iced_term::Command::ProxyToBackend(cmd))
    }

    /// Stream of terminal backend events (PTY output, exit, …).
    pub fn subscription(&self) -> iced::Subscription<iced_term::Event> {
        self.term.subscription()
    }
}
