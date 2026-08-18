//! Topic subscription: app-side event forwarder loop and the command stream
//! bridging a gossip subscription into the actor's command multiplexer.

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use n0_future::Stream;
use tokio::sync::broadcast;
use tracing::debug;

use super::protocol::ProtoEvent;
use crate::api::{Command, Event};
use crate::proto::TopicId;

pub(super) async fn topic_subscriber_loop(
    topic_id: TopicId,
    sender: irpc::channel::mpsc::Sender<Event>,
    mut topic_events: broadcast::Receiver<ProtoEvent>,
) {
    loop {
        tokio::select! {
           biased;
           msg = topic_events.recv() => {
               let event = match msg {
                   Err(broadcast::error::RecvError::Closed) => {
                       debug!(topic = %topic_id.fmt_short(), "gossip: subscriber loop exiting — topic event channel closed");
                       break
                   }
                   Err(broadcast::error::RecvError::Lagged(_)) => {
                       if crate::gossip_debug::is_enabled() {
                           crate::gossip_debug::log_event(
                               "Lagged",
                               Some(&topic_id.fmt_short()),
                               None,
                               None,
                           );
                       }
                       Event::Lagged
                   }
                   Ok(event) => event.into(),
               };
               let kind = match &event {
                   crate::api::Event::NeighborUp(_) => "NeighborUp",
                   crate::api::Event::NeighborDown(_) => "NeighborDown",
                   crate::api::Event::Received(_) => "Received",
                   crate::api::Event::MissingMessages { .. } => "MissingMessages",
                   crate::api::Event::Lagged => "Lagged",
               };
               debug!(
                   topic = %topic_id.fmt_short(),
                   event_kind = kind,
                   "SUBSCRIBER_SEND_BEGIN: forwarding event to app-side irpc channel",
               );
               if sender.send(event).await.is_err() {
                   debug!(topic = %topic_id.fmt_short(), "gossip: subscriber loop exiting — app-side receiver dropped");
                   break;
               }
               debug!(
                   topic = %topic_id.fmt_short(),
                   event_kind = kind,
                   "SUBSCRIBER_SEND_OK: irpc send completed",
               );
           }
           _ = sender.closed() => {
               debug!(topic = %topic_id.fmt_short(), "gossip: subscriber loop exiting — subscription channel closed (app receiver dropped)");
               break
           }
        }
    }
}

/// A stream of commands for a gossip subscription.
type BoxedCommandReceiver =
    n0_future::stream::Boxed<Result<Command, irpc::channel::mpsc::RecvError>>;

#[derive(derive_more::Debug)]
pub(super) struct TopicCommandStream {
    topic_id: TopicId,
    #[debug("CommandStream")]
    stream: BoxedCommandReceiver,
    closed: bool,
}

impl TopicCommandStream {
    pub(super) fn new(topic_id: TopicId, stream: BoxedCommandReceiver) -> Self {
        Self {
            topic_id,
            stream,
            closed: false,
        }
    }
}

impl Stream for TopicCommandStream {
    type Item = (TopicId, Option<Command>);
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.closed {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.stream).poll_next(cx) {
            Poll::Ready(Some(Ok(item))) => Poll::Ready(Some((self.topic_id, Some(item)))),
            Poll::Ready(None) | Poll::Ready(Some(Err(_))) => {
                self.closed = true;
                Poll::Ready(Some((self.topic_id, None)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
