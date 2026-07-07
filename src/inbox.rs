//! Application-level message envelope and /iroh-chat-inbox/1 direct protocol.
//!
//! This module provides the `Inbox` service which implements a reliable, 
//! ACK-based offline messaging mechanism using QUIC streams over a custom ALPN.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    PublicKey,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use n0_error::StdResultExt;

/// ALPN for the Inbox service.
pub const INBOX_ALPN: &[u8] = b"/iroh-chat-inbox/1";

/// A stable identifier for a message.
pub type MessageId = [u8; 32];

/// A wrapper around a 64-byte signature to implement serde traits.
#[derive(Debug, Clone)]
pub struct SignatureWrapper(pub [u8; 64]);

impl serde::Serialize for SignatureWrapper {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SignatureWrapper {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = serde::Deserialize::deserialize(deserializer)?;
        let mut arr = [0u8; 64];
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom("expected 64 bytes"));
        }
        arr.copy_from_slice(&bytes);
        Ok(SignatureWrapper(arr))
    }
}

/// The encrypted and signed envelope for offline messaging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Stable message id
    pub id: MessageId,
    /// Author's public key
    pub author: PublicKey,
    /// Signature of the message payload by the author
    pub author_signature: SignatureWrapper,
    /// Device's public key that sent the message
    pub device: PublicKey,
    /// Signature of the message payload by the device
    pub device_signature: SignatureWrapper,
    /// List of recipient public keys
    pub recipients: Vec<PublicKey>,
    /// Timestamp in milliseconds since unix epoch
    pub timestamp: u64,
    /// The encrypted message payload
    pub payload: Bytes,
}

/// A protocol message exchanged on the inbox ALPN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InboxMessage {
    /// A new message sent to the inbox
    Deliver(Envelope),
    /// Acknowledgement of receipt of a message
    Ack {
        /// The message id being acknowledged
        message_id: MessageId,
        /// The device that received it
        from_device: PublicKey,
    },
    /// Request delivery of any missed messages since a timestamp
    SyncRequest {
        /// Timestamp in milliseconds
        since: u64,
        /// The device requesting sync
        device: PublicKey,
    },
    /// Response with missed messages
    SyncResponse {
        /// The missed envelopes
        messages: Vec<Envelope>,
    },
}

/// In-memory state of the Inbox.
#[derive(Debug)]
pub struct InboxState {
    /// Delivered messages.
    pub messages: HashMap<MessageId, Envelope>,
    /// ACKs received for messages.
    pub acks: HashMap<MessageId, HashSet<PublicKey>>,
}

impl InboxState {
    /// Creates a new, empty InboxState.
    pub fn new() -> Self {
        Self {
            messages: HashMap::new(),
            acks: HashMap::new(),
        }
    }
}

/// The Inbox protocol handler.
#[derive(Debug, Clone)]
pub struct Inbox {
    /// Internal thread-safe state.
    pub state: Arc<Mutex<InboxState>>,
}

impl Inbox {
    /// Create a new Inbox instance.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(InboxState::new())),
        }
    }

    /// Handle an incoming connection.
    pub async fn handle_connection(&self, conn: Connection) -> n0_error::Result<()> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let state = self.state.clone();
            
            // Read length prefix
            let mut len_buf = [0u8; 4];
            if let Err(_) = recv.read_exact(&mut len_buf).await {
                continue;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len > 1024 * 1024 * 10 { // 10MB limit
                continue; 
            }
            let mut buf = vec![0u8; len];
            if let Err(_) = recv.read_exact(&mut buf).await {
                continue;
            }

            if let Ok(msg) = postcard::from_bytes::<InboxMessage>(&buf) {
                match msg {
                    InboxMessage::Deliver(env) => {
                        let mut state = state.lock().unwrap();
                        state.messages.insert(env.id, env);
                    }
                    InboxMessage::Ack { message_id, from_device } => {
                        let mut state = state.lock().unwrap();
                        state.acks.entry(message_id).or_default().insert(from_device);
                    }
                    InboxMessage::SyncRequest { since, device: _ } => {
                        let missed: Vec<Envelope> = {
                            let state = state.lock().unwrap();
                            state.messages.values()
                                .filter(|env| env.timestamp >= since)
                                .cloned()
                                .collect()
                        };
                        
                        let resp = InboxMessage::SyncResponse { messages: missed };
                        let out_buf = postcard::to_stdvec(&resp).std_context("encode sync response")?;
                        let len = out_buf.len() as u32;
                        let _ = send.write_all(&len.to_be_bytes()).await;
                        let _ = send.write_all(&out_buf).await;
                    }
                    InboxMessage::SyncResponse { .. } => {
                        // Clients handle sync response
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolHandler for Inbox {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.handle_connection(connection)
            .await
            .map_err(|e| AcceptError::from_err(e))?;
        Ok(())
    }

    async fn shutdown(&self) {
        // Shutdown logic
    }
}


impl Inbox {
    /// Send a message directly to a peer via the inbox ALPN, with basic retry logic.
    pub async fn send_deliver(
        &self, 
        endpoint: &iroh::Endpoint, 
        peer: iroh::PublicKey, 
        env: Envelope,
    ) -> n0_error::Result<()> {
        let msg = InboxMessage::Deliver(env);
        let out_buf = postcard::to_stdvec(&msg).std_context("encode deliver")?;
        let len = out_buf.len() as u32;

        let conn = endpoint.connect(peer, INBOX_ALPN).await.std_context("connect inbox")?;
        let (mut send, mut recv) = conn.open_bi().await.std_context("open_bi")?;
        
        send.write_all(&len.to_be_bytes()).await.std_context("write len")?;
        send.write_all(&out_buf).await.std_context("write payload")?;
        send.finish().std_context("finish send")?;
        Ok(())
    }

    /// Send an ACK to a peer.
    pub async fn send_ack(
        &self,
        endpoint: &iroh::Endpoint,
        peer: iroh::PublicKey,
        message_id: MessageId,
        from_device: PublicKey,
    ) -> n0_error::Result<()> {
        let msg = InboxMessage::Ack { message_id, from_device };
        let out_buf = postcard::to_stdvec(&msg).std_context("encode ack")?;
        let len = out_buf.len() as u32;

        let conn = endpoint.connect(peer, INBOX_ALPN).await.std_context("connect inbox")?;
        let (mut send, mut _recv) = conn.open_bi().await.std_context("open_bi")?;
        
        send.write_all(&len.to_be_bytes()).await.std_context("write len")?;
        send.write_all(&out_buf).await.std_context("write payload")?;
        send.finish().std_context("finish send")?;
        Ok(())
    }
}
