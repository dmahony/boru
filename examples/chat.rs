use std::{collections::HashMap, fmt, io, net::{Ipv4Addr, SocketAddrV4}, str::FromStr};
#[cfg(feature = "tor-transport")]
use std::sync::Arc;

use bytes::Bytes;
use clap::Parser;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as CEvent, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, PublicKey,
    RelayMode, RelayUrl, SecretKey,
};
use iroh_gossip::{
    api::{Event, GossipReceiver},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_error::{bail_any, Result, StdResultExt};
use n0_future::{task, StreamExt};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;
#[cfg(feature = "tor-transport")]
use arti_client::{
    config::{TorClientConfig, TorClientConfigBuilder},
    BootstrapBehavior, TorClient,
};
#[cfg(feature = "tor-transport")]
use iroh::Watcher;
#[cfg(feature = "tor-transport")]
use iroh_gossip::tor_transport::TorTransport;
#[cfg(feature = "tor-transport")]
use std::{env, fs, path::PathBuf};
#[cfg(feature = "tor-transport")]
use tor_rtcompat::PreferredRuntime;

/// Chat over iroh-gossip
///
/// This broadcasts signed messages over iroh-gossip and verifies signatures
/// on received messages.
///
/// By default a new endpoint id is created when starting the example. To reuse your identity,
/// set the `--secret-key` flag with the secret key printed on a previous invocation.
///
/// By default, the relay server run by n0 is used. To use a local relay server, run
///     cargo run --bin iroh-relay --features iroh-relay -- --dev
/// in another terminal and then set the `-d http://localhost:3340` flag on this example.
#[derive(Parser, Debug)]
struct Args {
    /// secret key to derive our endpoint id from.
    #[clap(long)]
    secret_key: Option<String>,
    /// Set a custom relay server. By default, the relay server hosted by n0 will be used.
    #[clap(short, long)]
    relay: Option<RelayUrl>,
    /// Disable relay completely.
    #[clap(long)]
    no_relay: bool,
    /// Use Tor hidden services instead of direct iroh connectivity.
    #[cfg(feature = "tor-transport")]
    #[clap(long)]
    tor: bool,
    /// Set your nickname.
    #[clap(short, long)]
    name: Option<String>,
    /// Set the bind port for our socket. By default, a random port will be used.
    #[clap(long, default_value = "0")]
    bind_port: u16,
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    /// Open a chat room for a topic and print a ticket for others to join.
    ///
    /// If no topic is provided, a new topic will be created.
    Open {
        /// Optionally set the topic id (64 bytes, as hex string).
        topic: Option<TopicId>,
    },
    /// Join a chat room from a ticket.
    Join {
        /// The ticket, as base32 string.
        ticket: String,
    },
}

#[cfg(feature = "tor-transport")]
#[derive(Debug)]
struct TorStorageDirs {
    root: PathBuf,
    state_dir: PathBuf,
    cache_dir: PathBuf,
}

#[cfg(feature = "tor-transport")]
impl TorStorageDirs {
    fn new() -> Result<Self> {
        let root = env::temp_dir().join(format!(
            "iroh-gossip-chat-tor-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let state_dir = root.join("state");
        let cache_dir = root.join("cache");
        fs::create_dir_all(&state_dir)?;
        fs::create_dir_all(&cache_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            root,
            state_dir,
            cache_dir,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    #[cfg(feature = "tor-transport")]
    let use_tor = args.tor;
    #[cfg(not(feature = "tor-transport"))]
    let use_tor = false;

    // parse the cli command
    let (topic, peers) = match &args.command {
        Command::Open { topic } => {
            let topic = topic.unwrap_or_else(|| TopicId::from_bytes(rand::random()));
            println!("> opening chat room for topic {topic}");
            (topic, vec![])
        }
        Command::Join { ticket } => {
            let Ticket { topic, peers } = Ticket::from_str(ticket)?;
            println!("> joining chat room for topic {topic}");
            (topic, peers)
        }
    };

    // parse or generate our secret key
    let secret_key = match args.secret_key.as_ref() {
        None => SecretKey::generate(),
        Some(key) => key.parse()?,
    };
    println!(
        "> our secret key: {}",
        data_encoding::HEXLOWER.encode(&secret_key.to_bytes())
    );

    // configure our relay map
    // When Tor is used, default to disabled relays — Tor hidden services provide direct
    // connectivity without needing the iroh relay infrastructure.
    let relay_mode = match (use_tor, args.no_relay, args.relay.clone()) {
        (_, true, Some(_)) => bail_any!("You cannot set --no-relay and --relay at the same time"),
        (_, true, None) => RelayMode::Disabled,
        (true, false, None) => RelayMode::Disabled,
        (false, false, None) => RelayMode::Default,
        (_, false, Some(url)) => RelayMode::Custom(url.into()),
    };
    println!("> using relay servers: {}", fmt_relay_mode(&relay_mode));

    // create a memory lookup to pass in endpoint addresses to
    let memory_lookup = MemoryLookup::new();

    // build our iroh endpoint
    let (endpoint, transport_status_message, transport_notice_text, local_peer_addr) = {
        #[cfg(feature = "tor-transport")]
        if use_tor {
            let tor_dirs = TorStorageDirs::new()?;
            let (tor_client, tor_status_message) = bootstrap_tor(&tor_dirs).await?;
            let tor_transport = TorTransport::new(
                secret_key.public(),
                Arc::clone(&tor_client),
                args.bind_port,
            );
            let endpoint = Endpoint::builder(presets::N0DisableRelay)
                .secret_key(secret_key.clone())
                .address_lookup(memory_lookup.clone())
                .relay_mode(relay_mode.clone())
                .add_custom_transport(Arc::new(tor_transport.clone()))
                .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                .bind()
                .await?;
            endpoint.online().await;
            let local_peer_addr = tor_transport.watch_local_peer_addr().initialized().await;
            (
                endpoint,
                format!("Tor bootstrap finished: {tor_status_message}"),
                "Tor-backed custom transport is operational. Gossip messages are relayed over Tor hidden services."
                    .to_string(),
                local_peer_addr.endpoint_addr(),
            )
        } else {
            let endpoint = if matches!(relay_mode, RelayMode::Disabled) {
                Endpoint::builder(presets::N0DisableRelay)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            } else {
                Endpoint::builder(presets::N0)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            };
            if !matches!(relay_mode, RelayMode::Disabled) {
                endpoint.online().await;
            }
            let local_peer_addr = endpoint.addr();
            (
                endpoint,
                "> Direct iroh transport is ready.".to_string(),
                "Direct iroh transport is operational. Gossip messages use standard iroh connectivity."
                    .to_string(),
                local_peer_addr,
            )
        }
        #[cfg(not(feature = "tor-transport"))]
        {
            let endpoint = if matches!(relay_mode, RelayMode::Disabled) {
                Endpoint::builder(presets::N0DisableRelay)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            } else {
                Endpoint::builder(presets::N0)
                    .secret_key(secret_key.clone())
                    .address_lookup(memory_lookup.clone())
                    .relay_mode(relay_mode.clone())
                    .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
                    .bind()
                    .await?
            };
            if !matches!(relay_mode, RelayMode::Disabled) {
                endpoint.online().await;
            }
            let local_peer_addr = endpoint.addr();
            (
                endpoint,
                "> Direct iroh transport is ready.".to_string(),
                "Direct iroh transport is operational. Gossip messages use standard iroh connectivity."
                    .to_string(),
                local_peer_addr,
            )
        }
    };
    println!("> our endpoint id: {}", endpoint.id());

    // create the gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone());

    let ticket = Ticket {
        topic,
        peers: vec![local_peer_addr.clone()],
    };
    println!("> ticket to join us: {ticket}");

    // setup router
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    // join the gossip topic by connecting to known peers, if any
    let peer_ids = peers.iter().map(|peer| peer.id).collect::<Vec<_>>();
    let peer_count = peer_ids.len();
    for peer in &peers {
        memory_lookup.set_endpoint_info(peer.clone());
    }
    if peers.is_empty() {
        println!("> waiting for peers to join us...");
    } else {
        println!("> trying to connect to {} peers...", peers.len());
    };
    let (sender, receiver) = gossip.subscribe_and_join(topic, peer_ids).await?.split();
    println!("> connected!");

    let local_public = endpoint.secret_key().public();
    let local_label = args
        .name
        .clone()
        .unwrap_or_else(|| local_public.fmt_short().to_string());

    if let Some(name) = args.name.clone() {
        let message = Message::AboutMe { name };
        let encoded_message = SignedMessage::sign_and_encode(endpoint.secret_key(), &message)?;
        sender.broadcast(encoded_message).await?;
    }

    let mut app = AppState::new(StatusContext {
        transport_status: transport_status_message.clone(),
        topic,
        relay_mode: relay_mode.clone(),
        connected: true,
        peer_count: peer_count,
        identity_label: local_label.clone(),
        transport_notice: transport_notice_text.clone(),
    });
    app.push_system(transport_status_message);
    app.push_system(transport_notice_text);
    app.push_system(format!("Ticket to join this room: {ticket}"));
    if peers.is_empty() {
        app.push_system("Waiting for peers to join us...");
    } else {
        app.push_system(format!(
            "Trying to connect to {} peers from the ticket...",
            peers.len()
        ));
    }
    app.push_system("Controls: Enter send • Ctrl-C or Esc quit • PgUp/PgDn scroll history");
    if let Some(name) = args.name.clone() {
        app.push_system(format!("You announced yourself as {name}."));
    }

    let mut names = HashMap::new();
    names.insert(local_public, local_label.clone());

    let _terminal_guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    terminal.draw(|frame| render_app(frame, &mut app))?;

    let (net_tx, mut net_rx) = tokio::sync::mpsc::unbounded_channel();
    task::spawn(forward_gossip_events(receiver, net_tx));

    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_input_thread(ui_tx);

    while !app.should_quit {
        tokio::select! {
            Some(event) = ui_rx.recv() => {
                let redraw = handle_ui_event(
                    event,
                    &mut app,
                    &sender,
                    endpoint.secret_key(),
                    &local_label,
                ).await?;
                if redraw {
                    terminal.draw(|frame| render_app(frame, &mut app))?;
                }
            }
            Some(event) = net_rx.recv() => {
                handle_net_event(event, &mut app, &mut names, local_public)?;
                terminal.draw(|frame| render_app(frame, &mut app))?;
            }
            else => break,
        }
    }

    router.shutdown().await.anyerr()?;

    Ok(())
}


#[derive(Debug)]
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}

#[derive(Clone, Debug)]
struct StatusContext {
    transport_status: String,
    topic: TopicId,
    relay_mode: RelayMode,
    connected: bool,
    peer_count: usize,
    identity_label: String,
    transport_notice: String,
}

#[derive(Clone, Debug)]
struct Composer {
    text: String,
    cursor: usize,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }
}

impl From<&str> for Composer {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
        }
    }
}

impl Composer {
    fn text(&self) -> &str {
        &self.text
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn cursor_column(&self) -> u16 {
        self.text[..self.cursor].chars().count() as u16
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = prev_char_boundary(&self.text, self.cursor);
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = next_char_boundary(&self.text, self.cursor);
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let start = prev_char_boundary(&self.text, self.cursor);
            self.text.drain(start..self.cursor);
            self.cursor = start;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            let end = next_char_boundary(&self.text, self.cursor);
            self.text.drain(self.cursor..end);
        }
    }

    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        text
    }
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(idx, _)| cursor + idx)
        .unwrap_or(text.len())
}

#[derive(Clone, Debug)]
enum ChatKind {
    System,
    Local,
    Remote,
}

#[derive(Clone, Debug)]
struct ChatEntry {
    kind: ChatKind,
    label: String,
    body: String,
}

impl ChatEntry {
    fn system(text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::System,
            label: "System".to_string(),
            body: text.into(),
        }
    }

    fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Local,
            label: label.into(),
            body: text.into(),
        }
    }

    fn remote(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Remote,
            label: label.into(),
            body: text.into(),
        }
    }

    fn to_line(&self) -> Line<'static> {
        let style = match self.kind {
            ChatKind::System => Style::default().fg(Color::DarkGray),
            ChatKind::Local => Style::default().fg(Color::Green),
            ChatKind::Remote => Style::default().fg(Color::Blue),
        };
        Line::from(vec![
            Span::styled(
                format!("[{}]", self.label),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(self.body.clone()),
        ])
    }
}

#[derive(Debug)]
struct AppState {
    status: StatusContext,
    entries: Vec<ChatEntry>,
    composer: Composer,
    follow_latest: bool,
    scroll_offset: u16,
    last_log_height: u16,
    should_quit: bool,
}

impl AppState {
    fn new(status: StatusContext) -> Self {
        Self {
            status,
            entries: Vec::new(),
            composer: Composer::default(),
            follow_latest: true,
            scroll_offset: 0,
            last_log_height: 10,
            should_quit: false,
        }
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.push_entry(ChatEntry::system(text), true);
    }

    fn push_local(&mut self, label: impl Into<String>, text: impl Into<String>) {
        self.push_entry(ChatEntry::local(label, text), true);
    }

    fn push_remote(&mut self, label: impl Into<String>, text: impl Into<String>) {
        self.push_entry(ChatEntry::remote(label, text), true);
    }

    fn push_entry(&mut self, entry: ChatEntry, follow_latest: bool) {
        self.entries.push(entry);
        if follow_latest {
            self.follow_latest = true;
        }
    }

    fn chat_text(&self) -> Text<'static> {
        if self.entries.is_empty() {
            Text::from(Line::from(vec![Span::styled(
                "No messages yet. Say hello.",
                Style::default().fg(Color::DarkGray),
            )]))
        } else {
            Text::from(
                self.entries
                    .iter()
                    .map(ChatEntry::to_line)
                    .collect::<Vec<_>>(),
            )
        }
    }

    fn max_scroll_offset(&self, visible_height: u16) -> u16 {
        let visible_height = visible_height as usize;
        self.entries.len().saturating_sub(visible_height) as u16
    }

    fn rendered_scroll_offset(&self, visible_height: u16) -> u16 {
        let max = self.max_scroll_offset(visible_height);
        if self.follow_latest {
            max
        } else {
            self.scroll_offset.min(max)
        }
    }

    fn scroll_up(&mut self, amount: u16, visible_height: u16) {
        let max = self.max_scroll_offset(visible_height);
        self.follow_latest = false;
        if self.scroll_offset == 0 {
            self.scroll_offset = max.saturating_sub(amount);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        }
    }

    fn scroll_down(&mut self, amount: u16, visible_height: u16) {
        let max = self.max_scroll_offset(visible_height);
        self.scroll_offset = self.scroll_offset.saturating_add(amount).min(max);
        self.follow_latest = self.scroll_offset >= max;
    }
}

#[derive(Debug)]
enum UiEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Paste(String),
}

#[derive(Debug)]
enum NetEvent {
    Message { from: PublicKey, message: Message },
    Closed,
    Error(String),
}

fn spawn_input_thread(ui_tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) {
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            let keep_running = match event {
                CEvent::Key(key) => ui_tx.send(UiEvent::Key(key)).is_ok(),
                CEvent::Resize(width, height) => ui_tx.send(UiEvent::Resize(width, height)).is_ok(),
                CEvent::Paste(text) => ui_tx.send(UiEvent::Paste(text)).is_ok(),
                _ => true,
            };
            if !keep_running {
                break;
            }
        }
    });
}

async fn forward_gossip_events(
    mut receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
) {
    while let Ok(Some(event)) = receiver.try_next().await {
        if let Event::Received(msg) = event {
            match SignedMessage::verify_and_decode(&msg.content) {
                Ok((from, message)) => {
                    if net_tx.send(NetEvent::Message { from, message }).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = net_tx.send(NetEvent::Error(err.to_string()));
                    return;
                }
            }
        }
    }
    let _ = net_tx.send(NetEvent::Closed);
}

async fn handle_ui_event(
    event: UiEvent,
    app: &mut AppState,
    sender: &iroh_gossip::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
) -> Result<bool> {
    match event {
        UiEvent::Key(key) => {
            handle_key_event(key, app, sender, secret_key, local_label).await?;
            Ok(true)
        }
        UiEvent::Resize(_, _) => Ok(true),
        UiEvent::Paste(text) => {
            app.composer.insert_str(&text);
            Ok(true)
        }
    }
}

async fn handle_key_event(
    key: KeyEvent,
    app: &mut AppState,
    sender: &iroh_gossip::api::GossipSender,
    secret_key: &SecretKey,
    local_label: &str,
) -> Result<()> {
    let visible_height = app.last_log_height;
    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            app.should_quit = true;
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            let submitted = app.composer.take();
            if !submitted.trim().is_empty() {
                let message = Message::Message {
                    text: submitted.clone(),
                };
                let encoded_message = SignedMessage::sign_and_encode(secret_key, &message)?;
                sender.broadcast(encoded_message).await?;
                app.push_local(local_label.to_string(), submitted);
            }
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => app.composer.backspace(),
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => app.composer.delete(),
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => app.composer.move_left(),
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => app.composer.move_right(),
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => app.composer.move_home(),
        KeyEvent {
            code: KeyCode::End, ..
        } => app.composer.move_end(),
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => app.scroll_up(visible_height.max(1) / 2, visible_height),
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => app.scroll_down(visible_height.max(1) / 2, visible_height),
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            app.composer.insert_char(ch);
        }
        _ => {}
    }

    Ok(())
}

fn handle_net_event(
    event: NetEvent,
    app: &mut AppState,
    names: &mut HashMap<PublicKey, String>,
    local_public: PublicKey,
) -> Result<()> {
    match event {
        NetEvent::Message { from, message } => match message {
            Message::AboutMe { name } => {
                names.insert(from, name.clone());
                if from != local_public {
                    app.push_system(format!("{} is now known as {}", from.fmt_short(), name));
                }
            }
            Message::Message { text } => {
                if from != local_public {
                    let name = names
                        .get(&from)
                        .cloned()
                        .unwrap_or_else(|| from.fmt_short().to_string());
                    app.push_remote(name, text);
                }
            }
        },
        NetEvent::Closed => {
            app.push_system("The gossip receiver closed.");
            app.should_quit = true;
        }
        NetEvent::Error(err) => {
            app.push_system(format!("Network error: {err}"));
            app.should_quit = true;
        }
    }
    Ok(())
}

fn render_app(frame: &mut Frame<'_>, app: &mut AppState) {
    let status_height = status_panel_height(&app.status);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Min(5),
            Constraint::Length(5),
        ])
        .split(frame.area());

    let status_block = Block::default()
        .title(Span::styled(
            "Status",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let status_lines = status_lines(&app.status);
    let status_paragraph = Paragraph::new(Text::from(status_lines))
        .block(status_block)
        .wrap(Wrap { trim: true });
    frame.render_widget(status_paragraph, layout[0]);

    let log_block = Block::default()
        .title(Span::styled(
            "Chat log",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));
    let log_inner = log_block.inner(layout[1]);
    app.last_log_height = log_inner.height;
    let log_scroll = app.rendered_scroll_offset(log_inner.height);
    let log_text = app.chat_text();
    let log_paragraph = Paragraph::new(log_text)
        .block(log_block)
        .wrap(Wrap { trim: false })
        .scroll((log_scroll, 0));
    frame.render_widget(log_paragraph, layout[1]);

    let composer_block = Block::default()
        .title(Span::styled(
            "Composer",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));
    let composer_inner = composer_block.inner(layout[2]);
    frame.render_widget(composer_block, layout[2]);
    let prompt = "> ";
    let composer_line = Line::from(vec![
        Span::styled(
            prompt,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.composer.text().to_string()),
    ]);
    let composer_paragraph =
        Paragraph::new(Text::from(vec![composer_line])).wrap(Wrap { trim: false });
    frame.render_widget(composer_paragraph, composer_inner);
    let cursor_x = composer_inner
        .x
        .saturating_add(prompt.len() as u16)
        .saturating_add(app.composer.cursor_column());
    frame.set_cursor_position((cursor_x, composer_inner.y));
}

fn status_panel_height(context: &StatusContext) -> u16 {
    let height = status_lines(context).len() as u16 + 2;
    height.clamp(6, 10)
}

fn status_lines(context: &StatusContext) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    vec![
        Line::from(vec![
            Span::styled("Transport", label_style),
            Span::raw(format!(": {}", context.transport_status)),
        ]),
        Line::from(vec![
            Span::styled("Topic", label_style),
            Span::raw(format!(": {}", context.topic)),
        ]),
        Line::from(vec![
            Span::styled("Identity", label_style),
            Span::raw(format!(": {}", context.identity_label)),
        ]),
        Line::from(vec![
            Span::styled("Relay", label_style),
            Span::raw(format!(": {}", fmt_relay_mode(&context.relay_mode))),
        ]),
        Line::from(vec![
            Span::styled("Peers", label_style),
            Span::raw(format!(
                ": {} known peers • connected: {}",
                context.peer_count, context.connected
            )),
        ]),
        Line::from(vec![
            Span::styled("Notice", label_style),
            Span::raw(format!(": {}", context.transport_notice)),
        ]),
        Line::from(vec![
            Span::styled("Controls", label_style),
            Span::raw(": Enter send • Ctrl-C or Esc quit • PgUp/PgDn scroll history"),
        ]),
    ]
}

const SIGNATURE_LENGTH: usize = iroh::Signature::LENGTH;
type Signature = ByteArray<SIGNATURE_LENGTH>;

#[derive(Debug, Serialize, Deserialize)]
struct SignedMessage {
    from: PublicKey,
    data: Bytes,
    signature: Signature,
}

impl SignedMessage {
    pub fn verify_and_decode(bytes: &[u8]) -> Result<(PublicKey, Message)> {
        let signed_message: Self =
            postcard::from_bytes(bytes).std_context("decode signed message")?;
        let key: PublicKey = signed_message.from;
        key.verify(
            &signed_message.data,
            &iroh::Signature::from_bytes(&signed_message.signature),
        )
        .std_context("verify signature")?;
        let message: Message =
            postcard::from_bytes(&signed_message.data).std_context("decode message")?;
        Ok((signed_message.from, message))
    }

    pub fn sign_and_encode(secret_key: &SecretKey, message: &Message) -> Result<Bytes> {
        let data: Bytes = postcard::to_stdvec(&message)
            .std_context("encode message")?
            .into();
        let signature = secret_key.sign(&data);
        let from: PublicKey = secret_key.public();
        let signed_message = Self {
            from,
            data,
            signature: ByteArray::new(signature.to_bytes()),
        };
        let encoded = postcard::to_stdvec(&signed_message).std_context("encode signed message")?;
        Ok(encoded.into())
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Message {
    AboutMe { name: String },
    Message { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Ticket {
    topic: TopicId,
    peers: Vec<EndpointAddr>,
}

impl Ticket {
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).std_context("decode chat ticket")
    }

    fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard::to_stdvec is infallible")
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
        text.make_ascii_lowercase();
        write!(f, "{text}")
    }
}

impl FromStr for Ticket {
    type Err = n0_error::AnyError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .std_context("decode chat ticket base32")?;
        Self::from_bytes(&bytes)
    }
}

// helpers

fn fmt_relay_mode(relay_mode: &RelayMode) -> String {
    match relay_mode {
        RelayMode::Disabled => "None".to_string(),
        RelayMode::Default => "Default Relay (production) servers".to_string(),
        RelayMode::Staging => "Default Relay (staging) servers".to_string(),
        RelayMode::Custom(map) => map
            .urls::<Vec<_>>()
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

#[cfg(feature = "tor-transport")]
fn format_tor_bootstrap_status_line(status: impl fmt::Display) -> String {
    format!("> Tor bootstrap status: {status}")
}

#[cfg(feature = "tor-transport")]
fn print_tor_bootstrap_status(status: impl fmt::Display, last_rendered: &mut Option<String>) {
    let rendered = format_tor_bootstrap_status_line(status);
    if last_rendered.as_deref() != Some(rendered.as_str()) {
        println!("{rendered}");
        *last_rendered = Some(rendered);
    }
}

#[cfg(feature = "tor-transport")]
fn tor_client_config(tor_dirs: &TorStorageDirs) -> Result<TorClientConfig> {
    TorClientConfigBuilder::from_directories(&tor_dirs.state_dir, &tor_dirs.cache_dir)
        .build()
        .std_context("build Arti Tor client config")
}

#[cfg(feature = "tor-transport")]
async fn bootstrap_tor(tor_dirs: &TorStorageDirs) -> Result<(Arc<TorClient<PreferredRuntime>>, String)> {
    let tor_config = tor_client_config(tor_dirs)?;
    let tor_client = TorClient::builder()
        .config(tor_config)
        .bootstrap_behavior(BootstrapBehavior::Manual)
        .create_unbootstrapped_async()
        .await
        .anyerr()?;

    let mut last_bootstrap_status = None;
    print_tor_bootstrap_status(tor_client.bootstrap_status(), &mut last_bootstrap_status);
    let mut bootstrap_events = tor_client.bootstrap_events();
    let mut bootstrap_task = {
        let tor_client = Arc::clone(&tor_client);
        tokio::spawn(async move { tor_client.bootstrap().await })
    };
    let mut bootstrap_task_done = false;

    loop {
        if tor_client.bootstrap_status().ready_for_traffic() {
            break;
        }

        if bootstrap_task_done {
            match bootstrap_events.next().await {
                Some(status) => print_tor_bootstrap_status(status, &mut last_bootstrap_status),
                None => break,
            }
            continue;
        }

        tokio::select! {
            result = &mut bootstrap_task => {
                match result {
                    Ok(Ok(())) => {
                        bootstrap_task_done = true;
                        print_tor_bootstrap_status(tor_client.bootstrap_status(), &mut last_bootstrap_status);
                    }
                    Ok(Err(err)) => return Err(err).std_context("Tor bootstrap task failed"),
                    Err(err) => return Err(err).std_context("join Tor bootstrap task"),
                }
            }
            maybe_status = bootstrap_events.next() => {
                if let Some(status) = maybe_status {
                    print_tor_bootstrap_status(status, &mut last_bootstrap_status);
                }
            }
        }
    }

    if !tor_client.bootstrap_status().ready_for_traffic() {
        bail_any!("Tor bootstrap finished without becoming ready for traffic");
    }

    Ok((tor_client, "> Tor is ready.".to_string()))
}

#[cfg(feature = "tor-transport")]
fn tor_transport_notice() -> String {
    "Tor-backed custom transport is operational. Gossip messages are relayed over Tor hidden services.".to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "tor-transport")]
    #[test]
    fn formats_bootstrap_status_line_with_tor_prefix() {
        assert_eq!(
            format_tor_bootstrap_status_line("31%: bootstrapping"),
            "> Tor bootstrap status: 31%: bootstrapping"
        );
    }

    #[test]
    fn ticket_roundtrips_through_base32() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([9u8; 32]),
            peers: vec![EndpointAddr::new(SecretKey::generate().public())],
        };
        let encoded = ticket.to_string();
        let decoded = Ticket::from_str(&encoded).expect("ticket should decode");
        assert_eq!(decoded, ticket);
    }

    #[test]
    fn composer_inserts_and_moves_cursor() {
        let mut composer = Composer::default();
        composer.insert_str("hi");
        composer.move_left();
        composer.insert_char('!');
        assert_eq!(composer.text(), "h!i");
        assert_eq!(composer.cursor(), 2);
    }

    #[test]
    fn composer_backspace_removes_character_before_cursor() {
        let mut composer = Composer::from("chat");
        composer.move_left();
        composer.move_left();
        composer.backspace();
        assert_eq!(composer.text(), "cat");
        assert_eq!(composer.cursor(), 1);
    }

    #[test]
    fn composer_take_clears_buffer() {
        let mut composer = Composer::from("hello");
        let submitted = composer.take();
        assert_eq!(submitted, "hello");
        assert!(composer.is_empty());
        assert_eq!(composer.cursor(), 0);
    }

    #[test]
    fn status_lines_include_transport_and_topic_context() {
        let status = StatusContext {
            transport_status: "Direct iroh transport is ready.".into(),
            topic: TopicId::from_bytes([7u8; 32]),
            relay_mode: RelayMode::Disabled,
            connected: true,
            peer_count: 3,
            identity_label: "alice".into(),
            transport_notice: "transport notice".into(),
        };
        let lines = status_lines(&status);
        let rendered: Vec<_> = lines.iter().map(|line| line.to_string()).collect();
        assert!(rendered.iter().any(|line| line.contains("Direct iroh transport is ready.")));
        assert!(rendered.iter().any(|line| line.contains("alice")));
        assert!(rendered.iter().any(|line| line.contains("3 known peers")));
    }

    #[test]
    fn cli_parses_direct_mode_by_default() {
        let args = Args::try_parse_from(["chat", "open"]).expect("direct mode should parse");
        assert!(matches!(args.command, Command::Open { .. }));
    }

    #[cfg(feature = "tor-transport")]
    #[test]
    fn tor_transport_notice_mentions_tor_operational() {
        let notice = tor_transport_notice();
        assert!(notice.contains("Tor-backed custom transport"));
        assert!(notice.contains("operational"));
    }

    #[cfg(feature = "tor-transport")]
    #[test]
    fn tor_client_config_builds_direct_tor_configuration() {
        let tor_dirs = TorStorageDirs::new().expect("test tor dirs should be creatable");
        let config = tor_client_config(&tor_dirs).expect("direct tor config should build");
        let _ = config;
    }

}
