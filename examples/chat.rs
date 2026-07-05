use std::{
    collections::HashMap,
    env,
    fs,
    fmt,
    io,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    str::FromStr,
};

use arti_client::{
    config::{pt::TransportConfigBuilder, BridgeConfigBuilder, CfgPath, TorClientConfig, TorClientConfigBuilder},
    BootstrapBehavior, TorClient,
};
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
use n0_error::{bail_any, AnyError, Result, StdResultExt};
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
    /// Set your nickname.
    #[clap(short, long)]
    name: Option<String>,
    /// Set the bind port for our socket. By default, a random port will be used.
    #[clap(long, default_value = "0")]
    bind_port: u16,
    /// Tor bridge line(s) to pass into Arti. Repeat this flag for multiple bridges.
    ///
    /// Example: `--bridge 'Bridge obfs4 host:port fingerprint cert=... iat-mode=0'`
    #[clap(long = "bridge", value_name = "BRIDGE_LINE", required = true)]
    bridges: Vec<String>,
    /// Path or command name for the obfs4proxy transport binary.
    #[clap(long, value_name = "PATH", default_value = "obfs4proxy")]
    obfs4proxy: String,
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

#[derive(Debug)]
struct TorStorageDirs {
    root: PathBuf,
    state_dir: PathBuf,
    cache_dir: PathBuf,
}

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

impl Drop for TorStorageDirs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    println!("> Tor bootstrap is required before chat.");
    let tor_dirs = TorStorageDirs::new()?;
    let tor_status_message = match tor_client_config(&args, &tor_dirs) {
        Ok(tor_config) => match (TorClient::builder()
            .config(tor_config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped_async()
            .await)
            .anyerr()
        {
            Ok(tor_client) => {
                let mut last_bootstrap_status = format_tor_bootstrap_status_line(tor_client.bootstrap_status());
                println!("{last_bootstrap_status}");
                let mut bootstrap_events = tor_client.bootstrap_events();
                let bootstrap = tor_client.bootstrap();
                tokio::pin!(bootstrap);
                loop {
                    if tor_client.bootstrap_status().ready_for_traffic() {
                        break;
                    }
                    tokio::select! {
                        result = &mut bootstrap => {
                            result.anyerr()?;
                            let rendered = format_tor_bootstrap_status_line(tor_client.bootstrap_status());
                            if last_bootstrap_status != rendered {
                                println!("{rendered}");
                                last_bootstrap_status = rendered;
                            }
                            break;
                        }
                        maybe_status = bootstrap_events.next() => {
                            if let Some(status) = maybe_status {
                                let rendered = format_tor_bootstrap_status_line(status);
                                if last_bootstrap_status != rendered {
                                    println!("{rendered}");
                                    last_bootstrap_status = rendered;
                                }
                            }
                        }
                    }
                }
                "> Tor is ready.".to_string()
            }
            Err(err) => {
                let message = format!("> Tor bootstrap failed: {err}");
                println!("{message}");
                message
            }
        },
        Err(err) => {
            let message = format!("> Tor config failed: {err}");
            println!("{message}");
            message
        }
    };
    println!("> {}", tor_transport_notice(&args));
    println!("> continuing into chat using the current iroh transport");

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
    let relay_mode = match (args.no_relay, args.relay.clone()) {
        (false, None) => RelayMode::Default,
        (false, Some(url)) => RelayMode::Custom(url.into()),
        (true, None) => RelayMode::Disabled,
        (true, Some(_)) => bail_any!("You cannot set --no-relay and --relay at the same time"),
    };
    println!("> using relay servers: {}", fmt_relay_mode(&relay_mode));

    // create a memory lookup to pass in endpoint addresses to
    let memory_lookup = MemoryLookup::new();

    // build our magic endpoint
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .address_lookup(memory_lookup.clone())
        .relay_mode(relay_mode.clone())
        .bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, args.bind_port))?
        .bind()
        .await?;
    println!("> our endpoint id: {}", endpoint.id());

    // create the gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // print a ticket that includes our own endpoint id and endpoint addresses
    if !matches!(relay_mode, RelayMode::Disabled) {
        // if we are expecting a relay, wait until we get a home relay
        // before moving on
        endpoint.online().await;
    }
    let ticket = {
        let me = endpoint.addr();
        let peers = peers.iter().cloned().chain([me]).collect();
        Ticket { topic, peers }
    };
    println!("> ticket to join us: {ticket}");

    // setup router
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    // join the gossip topic by connecting to known peers, if any
    let peer_ids = peers.iter().map(|p| p.id).collect();
    let peer_count = peers.len();
    if peers.is_empty() {
        println!("> waiting for peers to join us...");
    } else {
        println!("> trying to connect to {} peers...", peers.len());
        // add the peer addrs from the ticket to our endpoint's addressbook so that they can be dialed
        for peer in peers.iter().cloned() {
            memory_lookup.add_endpoint_info(peer);
        }
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
        tor_status: tor_status_message.clone(),
        topic,
        relay_mode: relay_mode.clone(),
        bridge_count: args.bridges.len(),
        connected: true,
        peer_count: peer_count,
        identity_label: local_label.clone(),
        transport_notice: tor_transport_notice(&args),
    });
    app.push_system(format!("Tor bootstrap finished: {}", tor_status_message));
    app.push_system(tor_transport_notice(&args));
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

fn format_tor_bootstrap_status_line(status: impl fmt::Display) -> String {
    format!("> Tor bootstrap status: {status}")
}

fn print_tor_bootstrap_status(status: impl fmt::Display, last_rendered: &mut Option<String>) {
    let rendered = format_tor_bootstrap_status_line(status);
    if last_rendered.as_deref() != Some(rendered.as_str()) {
        println!("{rendered}");
        *last_rendered = Some(rendered);
    }
}

fn tor_client_config(args: &Args, tor_dirs: &TorStorageDirs) -> Result<TorClientConfig> {
    if args.bridges.is_empty() {
        bail_any!(
            "the chat example requires at least one --bridge line to start Arti in bridge mode"
        );
    }

    let mut builder = TorClientConfigBuilder::from_directories(&tor_dirs.state_dir, &tor_dirs.cache_dir);

    for bridge_line in &args.bridges {
        let bridge: BridgeConfigBuilder =
            bridge_line.parse().std_context("parse Tor bridge line")?;
        builder.bridges().bridges().push(bridge);
    }

    let mut transport = TransportConfigBuilder::default();
    transport
        .protocols(vec!["obfs4"
            .parse()
            .std_context("parse obfs4 transport name")?])
        .path(CfgPath::new(args.obfs4proxy.clone().into()))
        .run_on_startup(true);
    builder.bridges().transports().push(transport);

    builder.build().std_context("build Arti Tor client config")
}

fn tor_transport_notice(args: &Args) -> String {
    format!(
        "Tor bootstrap succeeded via {} configured obfs4 bridge(s). iroh gossip still uses the current non-Tor transport.",
        args.bridges.len()
    )
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
    tor_status: String,
    topic: TopicId,
    relay_mode: RelayMode,
    bridge_count: usize,
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
            Span::styled("Tor", label_style),
            Span::raw(format!(": {}", context.tor_status)),
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
            Span::styled("Bridges", label_style),
            Span::raw(format!(
                ": {} obfs4 bridge(s) • {}",
                context.bridge_count, context.transport_notice
            )),
        ]),
        Line::from(vec![
            Span::styled("Controls", label_style),
            Span::raw(": Enter send • Ctrl-C or Esc quit • PgUp/PgDn scroll history"),
        ]),
    ]
}

async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
    // init a peerid -> name hashmap
    let mut names = HashMap::new();
    while let Some(event) = receiver.try_next().await? {
        if let Event::Received(msg) = event {
            let (from, message) = SignedMessage::verify_and_decode(&msg.content)?;
            match message {
                Message::AboutMe { name } => {
                    names.insert(from, name.clone());
                    println!("> {} is now known as {}", from.fmt_short(), name);
                }
                Message::Message { text } => {
                    let name = names
                        .get(&from)
                        .map_or_else(|| from.fmt_short().to_string(), String::to_string);
                    println!("{name}: {text}");
                }
            }
        }
    }
    Ok(())
}

fn input_loop(line_tx: tokio::sync::mpsc::Sender<String>) -> Result<()> {
    let mut buffer = String::new();
    let stdin = std::io::stdin(); // We get `Stdin` here.
    loop {
        stdin.read_line(&mut buffer).anyerr()?;
        line_tx.blocking_send(buffer.clone()).anyerr()?;
        buffer.clear();
    }
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

#[derive(Debug, Serialize, Deserialize)]
struct Ticket {
    topic: TopicId,
    peers: Vec<EndpointAddr>,
}
impl Ticket {
    /// Deserializes from bytes.
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).std_context("decode ticket")
    }
    /// Serializes to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard::to_stdvec is infallible")
    }
}

/// Serializes to base32.
impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
        text.make_ascii_lowercase();
        write!(f, "{text}")
    }
}

/// Deserializes from base32.
impl FromStr for Ticket {
    type Err = AnyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .std_context("decode ticket base32")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bootstrap_status_line_with_tor_prefix() {
        assert_eq!(
            format_tor_bootstrap_status_line("31%: bootstrapping"),
            "> Tor bootstrap status: 31%: bootstrapping"
        );
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
    fn status_lines_include_tor_and_topic_context() {
        let status = StatusContext {
            tor_status: "Tor is ready".into(),
            topic: TopicId::from_bytes([7u8; 32]),
            relay_mode: RelayMode::Disabled,
            bridge_count: 2,
            connected: true,
            peer_count: 3,
            identity_label: "alice".into(),
            transport_notice: "transport notice".into(),
        };
        let lines = status_lines(&status);
        let rendered: Vec<_> = lines.iter().map(|line| line.to_string()).collect();
        assert!(rendered.iter().any(|line| line.contains("Tor is ready")));
        assert!(rendered.iter().any(|line| line.contains("alice")));
        assert!(rendered.iter().any(|line| line.contains("3 known peers")));
    }

    #[test]
    fn cli_requires_at_least_one_bridge_line() {
        let err =
            Args::try_parse_from(["chat", "open"]).expect_err("missing bridge should be rejected");
        let rendered = err.to_string();
        assert!(
            rendered.contains("--bridge"),
            "unexpected parser error: {rendered}"
        );
    }

    #[test]
    fn tor_transport_notice_with_bridges_mentions_bridge_count() {
        let args = test_args(vec![valid_obfs4_bridge_line().to_string()]);
        let notice = tor_transport_notice(&args);
        assert!(notice.contains("1 configured obfs4 bridge"));
        assert!(notice.contains("iroh gossip still uses the current non-Tor transport"));
    }

    #[test]
    fn tor_client_config_accepts_obfs4_bridge_and_transport_configuration() {
        let tor_dirs = TorStorageDirs::new().expect("test tor dirs should be creatable");
        let args = test_args(vec![valid_obfs4_bridge_line().to_string()]);
        let config = tor_client_config(&args, &tor_dirs).expect("bridge config should build");
        let _ = config;
    }

    fn test_args(bridges: Vec<String>) -> Args {
        Args {
            secret_key: None,
            relay: None,
            no_relay: false,
            name: None,
            bind_port: 0,
            bridges,
            obfs4proxy: "obfs4proxy".to_string(),
            command: Command::Open { topic: None },
        }
    }

    fn valid_obfs4_bridge_line() -> &'static str {
        "Bridge obfs4 192.0.2.55:38114 316E643333645F6D79216558614D3931657A5F5F cert=YXJlIGZyZXF1ZW50bHkgZnVsbCBvZiBsaXR0bGUgbWVzc2FnZXMgeW91IGNhbiBmaW5kLg iat-mode=0"
    }
}
