//! BORU-TWEMOJI-03: minimal Boru view proving one bundled Twemoji SVG renders
//! at multiple widget sizes with the existing iced 0.14 `svg` feature.
//!
//! Run:  cargo run --example svg_render_proof --features gui
//! (or:  rb run --example svg_render_proof --features gui  on DEBSRV)
//!
//! Shows `assets/emoji/twemoji/svg/1f600.svg` (U+1F600 grinning face) at
//! 16/32/64/128 px. This is the reference render proof for the emoji picker
//! conversion (BORU-TWEMOJI-10): the widget is built with
//! `iced::widget::svg::Svg::new(handle).width(..).height(..)` — the same API
//! the picker will use via the shared renderer.
//!
//! NOTE: unlike `app.rs::icon_svg`, no `svg::Style` colour filter is applied
//! here — Twemoji artwork is multi-colour and must render with its intrinsic
//! colours.

use iced::widget::{column, container, scrollable, svg, text};
use iced::{Element, Length};

/// State for the proof view — nothing to hold yet.
#[derive(Debug, Clone)]
pub enum Message {}

/// Boot: no state, no startup task.
fn boot() -> ((), iced::Task<Message>) {
    ((), iced::Task::none())
}

/// No interactions in the proof view.
fn update(_state: &mut (), _message: Message) -> iced::Task<Message> {
    iced::Task::none()
}

/// One column of labelled Twemoji SVGs at increasing sizes.
fn view(_state: &()) -> Element<'_, Message> {
    // Loaded once from the vendored asset set (BORU-TWEMOJI-02). Later tasks
    // will cache handles per codepoint (BORU-TWEMOJI-09) instead of per frame.
    let handle = svg::Handle::from_path(format!(
        "{}/assets/emoji/twemoji/svg/1f600.svg",
        env!("CARGO_MANIFEST_DIR")
    ));

    let sizes: [f32; 4] = [16.0, 32.0, 64.0, 128.0];
    let mut col =
        column![text("Boru SVG render proof — twemoji/svg/1f600.svg (U+1F600)").size(14),]
            .spacing(12)
            .padding(20);

    for size in sizes {
        let emoji = svg::Svg::new(handle.clone())
            .width(Length::Fixed(size))
            .height(Length::Fixed(size));
        col = col.push(
            container(column![emoji, text(format!("{size}px")).size(12)].spacing(4)).padding(8),
        );
    }

    scrollable(col).into()
}

fn main() -> iced::Result {
    iced::application(boot, update, view)
        .title("Boru SVG render proof")
        .theme(|_: &()| iced::Theme::Dark)
        .run()
}
