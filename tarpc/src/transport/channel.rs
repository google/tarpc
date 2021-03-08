// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Transports backed by in-memory channels.

use crate::PollIo;
use futures::{task::*, Sink, Stream};
use pin_project::pin_project;
use std::io;
use std::pin::Pin;
use tokio::sync::mpsc;

/// Returns two unbounded channel peers. Each [`Stream`] yields items sent through the other's
/// [`Sink`].
pub fn unbounded<SinkItem, Item>() -> (
    UnboundedChannel<SinkItem, Item>,
    UnboundedChannel<Item, SinkItem>,
) {
    let (tx1, rx2) = mpsc::unbounded_channel();
    let (tx2, rx1) = mpsc::unbounded_channel();
    (
        UnboundedChannel { tx: tx1, rx: rx1 },
        UnboundedChannel { tx: tx2, rx: rx2 },
    )
}

/// A bi-directional channel backed by an [`UnboundedSender`](mpsc::UnboundedSender)
/// and [`UnboundedReceiver`](mpsc::UnboundedReceiver).
#[pin_project]
#[derive(Debug)]
pub struct UnboundedChannel<Item, SinkItem> {
    #[pin]
    rx: mpsc::UnboundedReceiver<Item>,
    #[pin]
    tx: mpsc::UnboundedSender<SinkItem>,
}

impl<Item, SinkItem> Stream for UnboundedChannel<Item, SinkItem> {
    type Item = Result<Item, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<Item> {
        self.project().rx.poll_recv(cx).map(|option| option.map(Ok))
    }
}

impl<Item, SinkItem> Sink<SinkItem> for UnboundedChannel<Item, SinkItem> {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(if self.project().tx.is_closed() {
            Err(io::Error::from(io::ErrorKind::NotConnected))
        } else {
            Ok(())
        })
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        self.project()
            .tx
            .send(item)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // UnboundedSender requires no flushing.
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // UnboundedSender can't initiate closure.
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
#[cfg(feature = "tokio1")]
mod tests {
    use crate::{
        client, context,
        server::{BaseChannel, Incoming},
        transport,
    };
    use assert_matches::assert_matches;
    use futures::{prelude::*, stream};
    use log::trace;
    use std::io;

    #[tokio::test]
    async fn integration() -> io::Result<()> {
        let _ = env_logger::try_init();

        let (client_channel, server_channel) = transport::channel::unbounded();
        tokio::spawn(
            stream::once(future::ready(server_channel))
                .map(BaseChannel::with_defaults)
                .execute(|_ctx, request: String| {
                    future::ready(request.parse::<u64>().map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{:?} is not an int", request),
                        )
                    }))
                }),
        );

        let client = client::new(client::Config::default(), client_channel).spawn()?;

        let response1 = client.call(context::current(), "123".into()).await?;
        let response2 = client.call(context::current(), "abc".into()).await?;

        trace!("response1: {:?}, response2: {:?}", response1, response2);

        assert_matches!(response1, Ok(123));
        assert_matches!(response2, Err(ref e) if e.kind() == io::ErrorKind::InvalidInput);

        Ok(())
    }
}
