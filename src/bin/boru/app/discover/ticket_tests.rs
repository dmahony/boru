//! Real ticket handler/task and native widget regressions; no public network.
use super::*;
use crate::app::tests::build_prewarm_test_app;
use iced::advanced::{mouse, widget::Operation, Layout, Shell};
use iced::{Event, Rectangle};

fn ticket() -> String {
    Ticket {
        topic: TopicId::from_bytes([0x79; 32]),
        peers: Vec::new(),
        discovery_secret: None,
    }
    .to_string()
}

#[test]
fn discover_ticket_invalid_is_inline_and_correctable() {
    let (_runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    let generation = app.room_generation;
    for input in ["", " \t\n", "not-a-ticket"] {
        drop(app.update(AppMessage::DiscoverTicketInputChanged(input.into())));
        assert!(app.discover_ticket_error.is_empty());
        assert_eq!(app.update(AppMessage::DiscoverJoinFromTicket).units(), 0);
        assert_eq!(app.discover_ticket_input, input);
        assert!(!app.discover_ticket_error.is_empty());
        assert_eq!(app.discover_ticket_pending, None);
        assert_eq!(app.room_generation, generation);
        assert_eq!(app.screen, Screen::Discover);
        assert!(!app.room_loading);
    }
    drop(app.update(AppMessage::DiscoverTicketInputChanged(ticket())));
    assert!(app.discover_ticket_error.is_empty());
    assert!(app.update(AppMessage::DiscoverJoinFromTicket).units() > 0);
}

fn completion(runtime: &tokio::runtime::Runtime, task: iced::Task<AppMessage>) -> AppMessage {
    runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).expect("real join task");
        match tokio::time::timeout(
            Duration::from_secs(10),
            iced::futures::StreamExt::next(&mut stream),
        )
        .await
        .expect("join task completed")
        .expect("completion action")
        {
            iced_runtime::Action::Output(message) => message,
            _ => panic!("expected the shared join completion"),
        }
    })
}

#[test]
fn discover_ticket_real_join_prevents_duplicates_and_opens_only_on_completion() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.private_dht_disabled = true;
    let draft = format!("  {}  ", ticket());
    drop(app.update(AppMessage::DiscoverTicketInputChanged(draft.clone())));
    let idle = fxhash_of(&app.discover_dependency());
    let task = app.update(AppMessage::DiscoverJoinFromTicket);
    let generation = app.room_generation;
    assert_eq!(app.discover_ticket_pending, Some(generation));
    assert!(app.room_loading);
    assert_ne!(idle, fxhash_of(&app.discover_dependency()));
    assert_eq!(app.screen, Screen::Discover);
    assert_eq!(app.update(AppMessage::DiscoverJoinFromTicket).units(), 0);
    assert_eq!(app.update(AppMessage::JoinFromTicket).units(), 0);
    drop(app.update(AppMessage::DiscoverTicketInputChanged("replacement".into())));
    assert_eq!(app.discover_ticket_input, draft);
    assert_eq!(app.room_generation, generation);
    let message = completion(&runtime, task);
    assert!(matches!(message, AppMessage::RoomOpened { .. }));
    runtime.block_on(async {
        drop(app.update(message));
    });
    assert_eq!(app.discover_ticket_pending, None);
    assert!(!app.room_loading);
    assert!(app.discover_ticket_error.is_empty());
    assert_eq!(
        app.screen,
        Screen::Chat {
            topic: TopicId::from_bytes([0x79; 32])
        }
    );
}

#[test]
fn discover_ticket_real_failure_clears_pending_and_preserves_retry_text() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.private_dht_disabled = true;
    let draft = ticket();
    drop(app.update(AppMessage::DiscoverTicketInputChanged(draft.clone())));
    let task = app.update(AppMessage::DiscoverJoinFromTicket);
    // Shut down the actual gossip service, rather than invent a network result.
    runtime
        .block_on(app._router.clone().shutdown())
        .expect("shutdown router");
    let message = completion(&runtime, task);
    assert!(matches!(message, AppMessage::RoomJoinFailed { .. }));
    drop(app.update(message));
    assert_eq!(app.discover_ticket_pending, None);
    assert!(!app.room_loading);
    assert!(!app.discover_ticket_error.is_empty());
    assert_eq!(app.discover_ticket_input, draft);
    assert_eq!(app.screen, Screen::Discover);
    drop(app.update(AppMessage::DiscoverTicketInputChanged(ticket())));
    assert!(app.discover_ticket_error.is_empty());
    assert!(app.update(AppMessage::DiscoverJoinFromTicket).units() > 0);
}

#[test]
fn discover_ticket_stable_invite_uses_shared_join_and_reserved_topic_is_recoverable() {
    let (runtime, mut app) = build_prewarm_test_app();
    app.screen = Screen::Discover;
    app.private_dht_disabled = true;
    let reserved = Ticket {
        topic: boru_core::discovery_topic::BORU_DISCOVERY_TOPIC_V1,
        peers: Vec::new(),
        discovery_secret: None,
    }
    .to_string();
    drop(app.update(AppMessage::DiscoverTicketInputChanged(reserved.clone())));
    assert_eq!(app.update(AppMessage::DiscoverJoinFromTicket).units(), 0);
    assert_eq!(app.discover_ticket_pending, None);
    assert_eq!(app.discover_ticket_input, reserved);
    assert!(!app.discover_ticket_error.is_empty());
    assert!(!app.room_loading);
    let invite = RoomInviteV2::new(
        TopicId::from_bytes([0x7a; 32]),
        DiscoverySecret::from_bytes([0x12; 32]),
    )
    .encode();
    drop(app.update(AppMessage::DiscoverTicketInputChanged(invite)));
    let task = app.update(AppMessage::DiscoverJoinFromTicket);
    let message = completion(&runtime, task);
    assert!(matches!(message, AppMessage::RoomOpened { .. }));
    runtime.block_on(async {
        drop(app.update(message));
    });
    assert_eq!(app.discover_ticket_pending, None);
    assert!(!app.room_loading);
    assert!(app.discover_ticket_error.is_empty());
    assert_eq!(
        app.screen,
        Screen::Chat {
            topic: TopicId::from_bytes([0x7a; 32])
        }
    );
}

#[test]
fn discover_ticket_stale_failure_does_not_clobber_newer_room() {
    let (_runtime, mut app) = build_prewarm_test_app();
    app.discover_ticket_input = ticket();
    drop(app.update(AppMessage::DiscoverJoinFromTicket));
    let generation = app.room_generation;
    app.room_generation += 1;
    let screen = Screen::Chat {
        topic: TopicId::from_bytes([0x78; 32]),
    };
    app.screen = screen.clone();
    drop(app.update(AppMessage::RoomJoinFailed {
        error: "old failure".into(),
        generation,
    }));
    assert_eq!(app.discover_ticket_pending, None);
    assert_eq!(app.screen, screen);
    assert!(app.room_loading, "newer request still owns loading");
    // An old completion cannot settle a newer Discover request.
    app.discover_ticket_pending = Some(app.room_generation);
    drop(app.update(AppMessage::RoomJoinFailed {
        error: "older".into(),
        generation,
    }));
    assert_eq!(app.discover_ticket_pending, Some(app.room_generation));
}

#[derive(Default)]
struct Focus {
    target: usize,
    bounds: Vec<Rectangle>,
}
impl Operation for Focus {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
        visit(self);
    }
    fn focusable(
        &mut self,
        _: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if self.bounds.len() == self.target {
            state.focus();
        } else {
            state.unfocus();
        }
        self.bounds.push(bounds);
    }
}

fn widget_event(
    dep: &DiscoverDependency,
    width: f32,
    target: usize,
    pointer: bool,
) -> (Vec<AppMessage>, Vec<Rectangle>) {
    let mut element = IcedChat::discover_ticket_panel(dep);
    let renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(width, 1000.0)),
    );
    let mut focus = Focus {
        target,
        ..Default::default()
    };
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut focus);
    let mut messages = Vec::new();
    let (events, cursor) = if pointer {
        (
            vec![
                Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
                Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )),
            ],
            mouse::Cursor::Available(focus.bounds[target].center()),
        )
    } else {
        use iced::keyboard::{key, Key, Location, Modifiers};
        (
            vec![Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(key::Named::Enter),
                modified_key: Key::Named(key::Named::Enter),
                physical_key: key::Physical::Code(key::Code::Enter),
                location: Location::Standard,
                modifiers: Modifiers::default(),
                text: None,
                repeat: false,
            })],
            mouse::Cursor::Unavailable,
        )
    };
    for event in events {
        element.as_widget_mut().update(
            &mut tree,
            &event,
            Layout::new(&node),
            cursor,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(Size::new(width, 1000.0)),
        );
    }
    (messages, focus.bounds)
}

#[test]
fn discover_ticket_enter_and_button_share_action_and_blank_pending_are_disabled() {
    let mut dep = snapshot();
    dep.ticket_input = ticket();
    for width in [240.0, 720.0] {
        for (target, pointer) in [(0, false), (1, false), (1, true)] {
            let (messages, _) = widget_event(&dep, width, target, pointer);
            assert!(matches!(
                messages.as_slice(),
                [AppMessage::DiscoverJoinFromTicket]
            ));
        }
    }
    for (input, pending) in [("", false), ("   ", false), ("ticket", true)] {
        dep.ticket_input = input.into();
        dep.ticket_pending = pending;
        let (messages, _) = widget_event(&dep, 240.0, 0, false);
        assert!(messages.is_empty());
    }
    dep.ticket_pending = false;
    dep.ticket_blocked = true;
    let (messages, _) = widget_event(&dep, 240.0, 0, false);
    assert!(messages.is_empty());
}

#[test]
fn discover_ticket_long_input_is_contained_and_controls_stack_at_narrow_widths() {
    let mut dep = snapshot();
    dep.ticket_input = "abcdefgh".repeat(2048);
    for width in [180.0, 240.0, 320.0, 720.0, 1000.0] {
        let panel = measure(IcedChat::discover_ticket_panel(&dep), width, 1000.0);
        assert!(
            panel.size().height < 300.0,
            "ticket panel must stay compact"
        );
        let (_, bounds) = widget_event(&dep, width, 0, false);
        assert_eq!(bounds.len(), 2);
        for b in &bounds {
            assert!(b.x >= 0.0 && b.x + b.width <= width);
        }
        if width < 420.0 {
            assert!(bounds[1].y >= bounds[0].y + bounds[0].height);
        } else {
            assert!((bounds[0].y - bounds[1].y).abs() < 1.0);
            assert!((bounds[0].height - bounds[1].height).abs() < 1.0);
        }
    }
    dep.ticket_error = "invalid-ticket-without-spaces".repeat(20);
    let node = measure(IcedChat::discover_ticket_panel(&dep), 180.0, f32::INFINITY);
    assert_eq!(node.size().width, 180.0);
    assert!(node.size().height > 100.0);
}
