//! Embedded dashboard artwork. Stable handles reuse Iced's decoded-image cache.
use std::sync::OnceLock;

use iced::widget::image::{self, Handle};
use iced::{ContentFit, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeArtwork {
    NewChat,
    PublicRooms,
    CreateRoom,
    ShareFile,
    QuickActionsBolt,
    Tunnels,
}

impl HomeArtwork {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::NewChat => include_bytes!("../../../assets/home/new_chat.png"),
            Self::PublicRooms => include_bytes!("../../../assets/home/public_rooms.png"),
            Self::CreateRoom => include_bytes!("../../../assets/home/create_room.png"),
            Self::ShareFile => include_bytes!("../../../assets/home/share_file.png"),
            Self::QuickActionsBolt => include_bytes!("../../../assets/home/quick_actions_bolt.png"),
            Self::Tunnels => include_bytes!("../../../assets/home/tunnels_illustration.png"),
        }
    }

    fn handle(self) -> Handle {
        static HANDLES: [OnceLock<Handle>; 6] = [const { OnceLock::new() }; 6];
        HANDLES[self as usize]
            .get_or_init(|| Handle::from_bytes(self.bytes()))
            .clone()
    }

    pub(crate) fn image(self, width: f32, height: f32) -> image::Image<Handle> {
        image::Image::new(self.handle())
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .content_fit(ContentFit::Contain)
    }
}

#[cfg(test)]
mod tests {
    use super::HomeArtwork;

    #[test]
    fn home_artwork_tunnel_empty_state_fits_narrow_and_desktop() {
        use iced::advanced::{layout, mouse::Cursor, renderer::Headless, widget::Tree};
        use iced::{Font, Pixels, Rectangle, Size};
        use std::borrow::Cow;

        for bytes in [
            include_bytes!("fonts/PublicSans-Regular.ttf").as_slice(),
            include_bytes!("fonts/PublicSans-SemiBold.ttf").as_slice(),
            include_bytes!("fonts/InterTight-Bold.ttf").as_slice(),
        ] {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(Cow::Borrowed(bytes));
        }
        for width in [300, 420] {
            for dark in [false, true] {
                let dep = crate::app::TunnelsCardData {
                    dark_mode: dark,
                    theme_revision: 0,
                    tick: 0,
                    rows: vec![],
                    compact_header: width < 360,
                    home_menu_item_opacity_bits: 1.0_f32.to_bits(),
                };
                let mut element = crate::app::IcedChat::view_tunnels_card(
                    &dep,
                    crate::theme::BoruTheme::default(),
                    width as f32,
                );
                let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
                    Font::default(),
                    Pixels(16.0),
                ));
                let mut tree = Tree::new(element.as_widget());
                let size = Size::new(width as f32, 500.0);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(Size::ZERO, size),
                );
                fn check_width(node: &layout::Node, parent_width: f32) {
                    assert!(node.bounds().width <= parent_width + 0.1);
                    for child in node.children() {
                        check_width(child, node.bounds().width);
                    }
                }
                check_width(&node, width as f32);
                let theme = if dark {
                    iced::Theme::Dark
                } else {
                    iced::Theme::Light
                };
                element.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &theme,
                    &iced::advanced::renderer::Style::default(),
                    iced::advanced::Layout::new(&node),
                    Cursor::default(),
                    &Rectangle::with_size(size),
                );
                let rgba = renderer.screenshot(
                    Size::new(width, 500),
                    1.0,
                    crate::design_tokens::surface(&theme),
                );
                if let Ok(dir) = std::env::var("CAPTURE_DIR") {
                    std::fs::create_dir_all(&dir).unwrap();
                    ::image::save_buffer_with_format(
                        std::path::Path::new(&dir).join(format!("tunnels_{width}_{dark}.png")),
                        &rgba,
                        width,
                        500,
                        ::image::ExtendedColorType::Rgba8,
                        ::image::ImageFormat::Png,
                    )
                    .unwrap();
                }
            }
        }
    }

    #[test]
    fn home_artwork_is_embedded_transparent_and_cached() {
        for artwork in [
            HomeArtwork::NewChat,
            HomeArtwork::PublicRooms,
            HomeArtwork::CreateRoom,
            HomeArtwork::ShareFile,
            HomeArtwork::QuickActionsBolt,
            HomeArtwork::Tunnels,
        ] {
            let decoded = ::image::load_from_memory(artwork.bytes()).unwrap();
            let expected = if artwork == HomeArtwork::Tunnels {
                (1440, 512)
            } else {
                (256, 256)
            };
            assert_eq!((decoded.width(), decoded.height()), expected);
            assert!(decoded.color().has_alpha());
            assert_eq!(decoded.to_rgba8().get_pixel(0, 0)[3], 0);
            assert_eq!(artwork.handle().id(), artwork.handle().id());
        }
    }
}
