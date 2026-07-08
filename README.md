# iroh-gossip

This crate implements the `iroh-gossip` protocol.
It is based on *epidemic broadcast trees* to disseminate messages among a swarm of peers interested in a *topic*.
The implementation is based on the papers [HyParView](https://asc.di.fct.unl.pt/~jleitao/pdf/dsn07-leitao.pdf) and [PlumTree](https://asc.di.fct.unl.pt/~jleitao/pdf/srds07-leitao.pdf).

The crate is made up from two modules:
The `proto` module is the protocol implementation, as a state machine without any IO.
The `net` module implements networking logic for running `iroh-gossip` on `iroh` connections.

The `net` module is optional behind the `net` feature flag (enabled by default).

# Getting Started

The `iroh-gossip` protocol was designed to be used in conjunction with `iroh`. [Iroh](https://docs.rs/iroh) is a networking library for making direct connections, these connections are how gossip messages are sent.

Iroh provides a [`Router`](https://docs.rs/iroh/latest/iroh/protocol/struct.Router.html) that takes an [`Endpoint`](https://docs.rs/iroh/latest/iroh/endpoint/struct.Endpoint.html) and any protocols needed for the application. Similar to a router in webserver library, it runs a loop accepting incoming connections and routes them to the specific protocol handler, based on `ALPN`.

Here is a basic example of how to set up `iroh-gossip` with `iroh`:
```rust,no_run
use iroh::{protocol::Router, Endpoint, EndpointId, endpoint::presets};
use iroh_gossip::{api::Event, Gossip, TopicId};
use n0_error::{Result, StdResultExt};
use n0_future::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    // create an iroh endpoint that includes the standard discovery mechanisms
    // we've built at number0
    let endpoint = Endpoint::bind(presets::N0).await?;

    // build gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // setup router
    let router = Router::builder(endpoint)
        .accept(iroh_gossip::ALPN, gossip.clone())
        .spawn();

    // gossip swarms are centered around a shared "topic id", which is a 32 byte identifier
    let topic_id = TopicId::from_bytes([23u8; 32]);
    // and you need some bootstrap peers to join the swarm
    let bootstrap_peers = bootstrap_peers();

    // then, you can subscribe to the topic and join your initial peers
    let (sender, mut receiver) = gossip
        .subscribe(topic_id, bootstrap_peers)
        .await?
        .split();

    // you might want to wait until you joined at least one other peer:
    receiver.joined().await?;

    // then, you can broadcast messages to all other peers!
    sender.broadcast(b"hello world this is a gossip message".to_vec().into()).await?;

    // and read messages from others!
    while let Some(event) = receiver.next().await {
        match event? {
            Event::Received(message) => {
                println!("received a message: {:?}", std::str::from_utf8(&message.content));
            }
            _ => {}
        }
    }

    // clean shutdown makes sure that other peers are notified that you went offline
    router.shutdown().await.std_context("shutdown router")?;
    Ok(())
}

fn bootstrap_peers() -> Vec<EndpointId> {
    // insert your bootstrap peers here, or get them from your environment
    vec![]
}
```

The `examples/chat.rs` demo runs over direct iroh connectivity by default. It prints a base32 ticket containing the topic and endpoint addresses, so peers can join without Tor. If you want Tor hidden services instead, build with `--features tor-transport` and pass `--tor`.

## Identity Persistence

Both the `chat` and `setup` examples persist the node identity (secret key) to disk so your iroh peer ID remains stable across restarts.

**Storage location** (checked in order):
1. `$IROH_GOSSIP_CHAT_DATA_DIR/secret_key.txt` — if the env var is set
2. `$XDG_DATA_HOME/iroh-gossip-chat/secret_key.txt` — typical: `~/.local/share/iroh-gossip-chat/secret_key.txt`
3. `$HOME/.local/share/iroh-gossip-chat/secret_key.txt` — fallback when `XDG_DATA_HOME` is unset
4. `$LOCALAPPDATA/iroh-gossip-chat/secret_key.txt` — Windows
5. `./.iroh-gossip-chat/secret_key.txt` — current directory fallback

**File format:** The secret key is stored as lowercase hex-encoded bytes (64 hex chars) with a trailing newline. The file is created with restrictive permissions (`0o600`, owner read/write only) on Unix systems.

**Resetting the identity:** Delete the `secret_key.txt` file. The next run will generate a fresh keypair.

```text
rm ~/.local/share/iroh-gossip-chat/secret_key.txt
```

You can also set `IROH_GOSSIP_CHAT_DATA_DIR` to a different path to use a separate identity for different sessions.

**Overriding via CLI flag:** The `chat` example accepts `--secret-key <hex>` to use a specific key for one session without writing it to disk.

The examples use Cargo aliases (defined in `.cargo/config.toml`) for
convenience.  The long-form `cargo run --example ...` is always an
alternative.

To run two peers with the **TUI (ratatui)** frontend:
```text
# Terminal 1 — create a room
cargo chat open

# Terminal 2 — join with the printed ticket
cargo chat join <ticket>

# Long form (also works without the alias)
cargo run --example chat -- open

# Optional Tor mode
cargo run --features "examples tor-transport" --example chat -- --tor open
```

## GUI frontend (iced)

A **GUI** frontend built with [iced](https://iced.rs) is
available behind the `gui` feature flag.

**`iced-chat`** (modular, split across `main.rs` + `app.rs`):
```text
cargo iced-chat open                   # open a room
cargo iced-chat join <ticket>          # join a room

# Long form
cargo run --features gui --example iced_chat -- open
```

The GUI replicates the full chat feature set: text messages, file
sharing (`/send <path>`, `/download`), dark mode toggle, and a
scrolling chat log.  Networking runs in background tokio tasks with
events flowing into the iced event loop via a channel.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
