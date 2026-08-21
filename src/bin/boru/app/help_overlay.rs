//! Help-overlay domain — the reference implementation of the BORU-APP-002
//! domain message-routing pattern.
//!
//! ## Pattern (see `app/domain_pattern.md` for the full spec)
//!
//! Every extracted subsystem follows the same four-part shape:
//!
//! - **DomainState** — [`HelpOverlay`] owns the help overlay's only state
//!   (the `visible` flag). `IcedChat` holds one instance (`self.help_overlay`);
//!   there is no other help-overlay state anywhere in the app.
//! - **DomainMessage** — [`HelpMessage`] is the domain-scoped message enum.
//!   The App routes `AppMessage::ToggleHelp` (and the Escape-close path) to
//!   [`HelpOverlay::update`].
//! - **update()** — mutates only this domain's state and returns a typed
//!   [`HelpEvent`] describing the side effect the shell must apply (here:
//!   complete any pending GUI-test action). The domain never touches
//!   `gui_action_history` or other shell state directly.
//! - **view()** — builds the domain's portion of the UI. [`HelpOverlay::view`]
//!   composes the overlay `Stack` on top of the chat layer it is given, or
//!   passes the layer through untouched when hidden.
//!
//! The App shell stays the composer/router: it owns the domain instance,
//! routes top-level messages to it, applies the returned events, and composes
//! `view()` output into the screen. Startup/shutdown, route switching and the
//! global error surface remain in `app.rs` / `main.rs`.
//!
//! Invariants:
//! - The help overlay is presentation-only: toggling it never touches
//!   networking, discovery, storage or any other domain.
//! - `visible` exists in exactly one place (this struct) — never mirrored on
//!   `IcedChat` (PDF §14 "same state in both modules" stop condition).

use super::*;

/// DomainState — all state owned by the help-overlay domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpOverlay {
    visible: bool,
}

impl Default for HelpOverlay {
    fn default() -> Self {
        Self { visible: false }
    }
}

impl HelpOverlay {
    /// Create a closed help overlay.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the overlay is currently shown.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Apply one domain message.
    ///
    /// Only this domain's state is mutated. Side effects the shell must
    /// perform are returned as typed [`HelpEvent`]s; `None` means "no side
    /// effect needed" (e.g. closing an already-closed overlay).
    pub fn update(&mut self, msg: HelpMessage) -> Option<HelpEvent> {
        match msg {
            HelpMessage::Toggle => {
                self.visible = !self.visible;
                Some(HelpEvent::VisibilityChanged {
                    visible: self.visible,
                })
            }
            HelpMessage::Close => {
                if self.visible {
                    self.visible = false;
                    Some(HelpEvent::VisibilityChanged { visible: false })
                } else {
                    None
                }
            }
        }
    }

    /// Compose the domain's portion of the UI.
    ///
    /// When hidden the `chat_layer` passes through untouched; when visible the
    /// overlay `Stack` (backdrop + help panel) is layered on top of it.
    pub fn view<'a>(
        &'a self,
        chat_layer: iced::Element<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        if !self.visible {
            return chat_layer;
        }

        use iced::widget::Stack;
        use iced::{widget, Length};

        let backdrop = widget::button(widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .on_press(AppMessage::ToggleHelp)
            .style(move |t, _status| {
                let b = crate::theme::BoruTheme::for_theme(t);
                iced::widget::button::Style {
                    background: Some(iced::Background::Color(b.colors.dialog_backdrop)),
                    ..Default::default()
                }
            });

        let help_panel = widget::container(self.help_panel())
            .width(Length::Shrink)
            .height(Length::Shrink)
            .max_width(480.0)
            .max_height(600.0)
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    radius: SPACE_12.into(),
                    ..Default::default()
                },
                shadow: iced::Shadow {
                    color: crate::theme::BoruTheme::for_theme(t).colors.panel_shadow,
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });

        Stack::new()
            .push(chat_layer)
            .push(backdrop)
            .push(
                widget::container(help_panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The help panel content (commands, friends, messages, tips, footer).
    /// Presentation-only; never reads other domain state.
    fn help_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, text, Column, Space};
        use iced::{Alignment, Length};

        // ── Header: title + accessible close button ──
        let header = iced::widget::row![
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::CardTitle,
                crate::i18n::t("common.help")
            )
            .width(Length::Fill),
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "Close",
            ))
            .on_press(AppMessage::ToggleHelp)
            .padding(SPACE_4)
            .style(BUTTON_GHOST),
        ]
        .align_y(Alignment::Center)
        .spacing(SPACE_8);

        // ── Command reference sections ──
        let commands = Column::new()
            .spacing(SPACE_6)
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "── Commands ──",
                )
                .style(text_muted_style),
            )
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/send <path>    Share a file with peers",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/image <path>   Share an image inline",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/download       Fetch the last shared file",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/leave          Leave this room and delete from history",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/help           Toggle this menu",
            ))
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "── Friends ──",
                )
                .style(text_muted_style),
            )
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/friend add <pk> [alias]  Track a friend's online status",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/friend remove <pk|alias> Stop tracking a friend",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/friend list    List tracked friends and their status",
            ))
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "── Messages ──",
                )
                .style(text_muted_style),
            )
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/react <idx> <emoji>  Add a reaction (or /unreact to remove)",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/edit <idx> <text>   Edit a message",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/delete <idx>        Delete a message",
            ))
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "── Tips ──")
                    .style(text_muted_style),
            )
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "Type a message and press Enter to send.",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "Click Remove on a room in the chat list to remove it.",
            ));

        // ── Footer ──
        let report_bug_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "Report Bug",
        ))
        .on_press(AppMessage::ReportBug)
        .padding([SPACE_6, SPACE_12])
        .style(|t, _status| {
            let b = crate::theme::BoruTheme::for_theme(t);
            iced::widget::button::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    color: border_muted(t),
                    width: b.borders.hairline,
                    radius: b.radii.sm.into(),
                },
                // BORU-UI-03: the muted fallback grey rgb(0.6,0.6,0.6) is
                // captured by ColorTokens::glyph_muted_dark in both modes.
                text_color: text_muted_style(t)
                    .color
                    .unwrap_or(b.colors.glyph_muted_dark),
                ..Default::default()
            }
        });

        let footer = Column::new()
            .push(report_bug_btn)
            .push(button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Save Support Bundle")).on_press(AppMessage::SaveSupportBundle).padding([SPACE_6, SPACE_12]))
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(
                text(crate::i18n::t("chat.press_esc_close"))
                    .size(crate::fonts::TypeRole::SupportingText.size_px())
                    .style(text_muted_style),
            );

        let dialog_content = Column::new()
            .push(header)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(commands)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(footer)
            .spacing(SPACE_4)
            .padding(SPACE_24)
            .width(Length::Fill);

        container(
            crate::ui_components::gutter_scrollable(dialog_content)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style::default())
        .into()
    }
}

/// DomainMessage — messages the help-overlay domain understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpMessage {
    /// Flip the overlay open/closed (help button, `/help`, GUI-test toggle).
    Toggle,
    /// Force-close the overlay (Escape/back navigation). No-op when hidden.
    Close,
}

/// Typed events emitted by [`HelpOverlay::update`] for the shell to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpEvent {
    /// The overlay's visibility changed. The shell uses this to complete any
    /// pending GUI-test action that targeted the help toggle.
    VisibilityChanged { visible: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let overlay = HelpOverlay::new();
        assert!(!overlay.visible());
    }

    #[test]
    fn toggle_opens_and_closes() {
        let mut overlay = HelpOverlay::new();
        assert_eq!(
            overlay.update(HelpMessage::Toggle),
            Some(HelpEvent::VisibilityChanged { visible: true })
        );
        assert!(overlay.visible());
        assert_eq!(
            overlay.update(HelpMessage::Toggle),
            Some(HelpEvent::VisibilityChanged { visible: false })
        );
        assert!(!overlay.visible());
    }

    #[test]
    fn close_is_noop_when_hidden() {
        let mut overlay = HelpOverlay::new();
        assert_eq!(overlay.update(HelpMessage::Close), None);
        assert!(!overlay.visible());
    }

    #[test]
    fn close_hides_when_visible() {
        let mut overlay = HelpOverlay::new();
        overlay.update(HelpMessage::Toggle);
        assert_eq!(
            overlay.update(HelpMessage::Close),
            Some(HelpEvent::VisibilityChanged { visible: false })
        );
        assert!(!overlay.visible());
    }
}
