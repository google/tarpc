// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Transports backed by in-memory channels.

use std::{error::Error, pin::Pin};

use futures::{ready, task::*, FutureExt, Sink, SinkExt, Stream, StreamExt};
use pin_project::pin_project;

/// Errors that occur in the sending or receiving of messages over a channel.
#[derive(thiserror::Error, Debug)]
pub enum ChannelError {
    /// An error occurred sending over the channel.
    #[error("an error occurred sending over the channel")]
    Send(#[source] Box<dyn Error + Send + Sync + 'static>),
}

/// Returns two unbounded channel peers. Each [`Stream`] yields items sent through the other's
/// [`Sink`].
pub fn unbounded<SinkItem, Item>() -> (
    UnboundedChannel<SinkItem, Item>,
    UnboundedChannel<Item, SinkItem>,
) {
    let (tx1, rx2) = flume::unbounded();
    let (tx2, rx1) = flume::unbounded();
    (
        UnboundedChannel { tx: tx1, rx: rx1 },
        UnboundedChannel { tx: tx2, rx: rx2 },
    )
}

/// A bi-directional channel backed by an [`Sender`](flume::Sender)
/// and [`Receiver`](flume::Receiver).
#[derive(Debug)]
pub struct UnboundedChannel<Item, SinkItem> {
    rx: flume::Receiver<Item>,
    tx: flume::Sender<SinkItem>,
}

impl<Item, SinkItem> Stream for UnboundedChannel<Item, SinkItem> {
    type Item = Result<Item, ChannelError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, ChannelError>>> {
        Poll::Ready(match ready!(self.rx.recv_async().poll_unpin(cx)) {
            Ok(x) => Some(Ok(x)),
            Err(_) => None,
        })
    }
}

const CLOSED_MESSAGE: &str = "the channel is closed and cannot accept new items for sending";

impl<Item, SinkItem> Sink<SinkItem> for UnboundedChannel<Item, SinkItem> {
    type Error = ChannelError;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(if self.tx.is_disconnected() {
            Err(ChannelError::Send(CLOSED_MESSAGE.into()))
        } else {
            Ok(())
        })
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> Result<(), Self::Error> {
        self.tx
            .send(item)
            .map_err(|_| ChannelError::Send(CLOSED_MESSAGE.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // UnboundedSender requires no flushing.
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // UnboundedSender can't initiate closure.
        Poll::Ready(Ok(()))
    }
}

/// Returns two channel peers with buffer equal to `capacity`. Each [`Stream`] yields items sent
/// through the other's [`Sink`].
pub fn bounded<SinkItem: Send + Sync, Item: Send + Sync>(
    capacity: usize,
) -> (Channel<SinkItem, Item>, Channel<Item, SinkItem>) {
    let (tx1, rx2) = flume::bounded(capacity);
    let (tx2, rx1) = flume::bounded(capacity);
    (Channel { tx: tx1, rx: rx1 }, Channel { tx: tx2, rx: rx2 })
}

/// A bi-directional channel backed by a [`Sender`](flume::Sender)
/// and [`Receiver`](flume::Receiver).
#[pin_project]
#[derive(Debug)]
pub struct Channel<Item: Send + Sync, SinkItem: Send + Sync> {
    #[pin]
    rx: flume::Receiver<Item>,
    #[pin]
    tx: flume::Sender<SinkItem>,
}

// impl<Item, SinkItem> Channel<Item, SinkItem> {
//     fn project_() {
//
//     }
// }

impl<Item: Send + Sync, SinkItem: Send + Sync> Stream for Channel<Item, SinkItem> {
    type Item = Result<Item, ChannelError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, ChannelError>>> {
        self.project()
            .rx
            .stream()
            .poll_next_unpin(cx)
            .map(|option| option.map(Ok))
    }
}

impl<Item: Send + Sync, SinkItem: Send + Sync + 'static> Sink<SinkItem>
    for Channel<Item, SinkItem>
{
    type Error = ChannelError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .tx
            .sink()
            .poll_ready_unpin(cx)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> Result<(), Self::Error> {
        self.project()
            .tx
            .sink()
            .start_send_unpin(item)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .tx
            .sink()
            .poll_flush_unpin(cx)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .tx
            .sink()
            .poll_close_unpin(cx)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }
}

#[cfg(test)]
#[cfg(feature = "tokio1")]
mod tests {
    use std::io;

    use assert_matches::assert_matches;
    use futures::{prelude::*, stream};
    use tracing::trace;

    use crate::{
        client, context,
        server::{incoming::Incoming, BaseChannel},
        transport::{
            self,
            channel::{Channel, UnboundedChannel},
        },
    };

    #[test]
    fn ensure_is_transport() {
        fn is_transport<SinkItem, Item, T: crate::Transport<SinkItem, Item>>() {}
        is_transport::<(), (), UnboundedChannel<(), ()>>();
        is_transport::<(), (), Channel<(), ()>>();
    }

    #[tokio::test]
    async fn integration() -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        let (client_channel, server_channel) = transport::channel::unbounded();
        tokio::spawn(
            stream::once(future::ready(server_channel))
                .map(BaseChannel::with_defaults)
                .execute(|_ctx, request: String| {
                    future::ready(request.parse::<u64>().map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{request:?} is not an int"),
                        )
                    }))
                }),
        );

        let client = client::new(client::Config::default(), client_channel).spawn();

        let response1 = client.call(context::current(), "", "123".into()).await?;
        let response2 = client.call(context::current(), "", "abc".into()).await?;

        trace!("response1: {:?}, response2: {:?}", response1, response2);

        assert_matches!(response1, Ok(123));
        assert_matches!(response2, Err(ref e) if e.kind() == io::ErrorKind::InvalidInput);

        Ok(())
    }
}
