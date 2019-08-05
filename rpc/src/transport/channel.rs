// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Transports backed by in-memory channels.

use crate::PollIo;
use futures::{channel::mpsc, task::Context, Poll, Sink, Stream};
use pin_utils::unsafe_pinned;
use std::io;
use std::pin::Pin;

/// Returns two unbounded channel peers. Each [`Stream`] yields items sent through the other's
/// [`Sink`].
pub fn unbounded<SinkItem, Item>() -> (
    UnboundedChannel<SinkItem, Item>,
    UnboundedChannel<Item, SinkItem>,
) {
    let (tx1, rx2) = mpsc::unbounded();
    let (tx2, rx1) = mpsc::unbounded();
    (
        UnboundedChannel { tx: tx1, rx: rx1 },
        UnboundedChannel { tx: tx2, rx: rx2 },
    )
}

/// A bi-directional channel backed by an [`UnboundedSender`](mpsc::UnboundedSender)
/// and [`UnboundedReceiver`](mpsc::UnboundedReceiver).
#[derive(Debug)]
pub struct UnboundedChannel<Item, SinkItem> {
    rx: mpsc::UnboundedReceiver<Item>,
    tx: mpsc::UnboundedSender<SinkItem>,
}

impl<Item, SinkItem> UnboundedChannel<Item, SinkItem> {
    unsafe_pinned!(rx: mpsc::UnboundedReceiver<Item>);
    unsafe_pinned!(tx: mpsc::UnboundedSender<SinkItem>);
}

impl<Item, SinkItem> Stream for UnboundedChannel<Item, SinkItem> {
    type Item = Result<Item, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<Item> {
        self.rx().poll_next(cx).map(|option| option.map(Ok))
    }
}

impl<Item, SinkItem> Sink<SinkItem> for UnboundedChannel<Item, SinkItem> {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx()
            .poll_ready(cx)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        self.tx()
            .start_send(item)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.tx()
            .poll_flush(cx)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx()
            .poll_close(cx)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        client, context,
        server::{Handler, Server},
        transport,
    };
    use assert_matches::assert_matches;
    use futures::{prelude::*, stream};
    use log::trace;
    use std::io;

    #[runtime::test(runtime_tokio::Tokio)]
    async fn integration() -> io::Result<()> {
        let _ = env_logger::try_init();

        let (client_channel, server_channel) = transport::channel::unbounded();
        crate::spawn(
            Server::default()
                .incoming(stream::once(future::ready(server_channel)))
                .respond_with(|_ctx, request: String| {
                    future::ready(request.parse::<u64>().map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{:?} is not an int", request),
                        )
                    }))
                }),
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut client = client::new(client::Config::default(), client_channel).spawn()?;

        let response1 = client.call(context::current(), "123".into()).await?;
        let response2 = client.call(context::current(), "abc".into()).await?;

        trace!("response1: {:?}, response2: {:?}", response1, response2);

        assert_matches!(response1, Ok(123));
        assert_matches!(response2, Err(ref e) if e.kind() == io::ErrorKind::InvalidInput);

        Ok(())
    }
}
