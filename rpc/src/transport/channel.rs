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
    type SinkError = io::Error;

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

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::SinkError>> {
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
    use futures::compat::Executor01CompatExt;
    use futures::{prelude::*, stream};
    use log::trace;
    use std::io;

    #[test]
    fn integration() {
        let _ = env_logger::try_init();
        crate::init(tokio::executor::DefaultExecutor::current().compat());

        let (client_channel, server_channel) = transport::channel::unbounded();
        let server = Server::<String, u64>::default()
            .incoming(stream::once(future::ready(Ok(server_channel))))
            .respond_with(|_ctx, request| {
                future::ready(request.parse::<u64>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{:?} is not an int", request),
                    )
                }))
            });

        let responses = async {
            let mut client = client::new(client::Config::default(), client_channel).await?;

            let response1 = client.call(context::current(), "123".into()).await;
            let response2 = client.call(context::current(), "abc".into()).await;

            Ok::<_, io::Error>((response1, response2))
        };

        let (response1, response2) = run_future(future::join(
            server,
            responses.unwrap_or_else(|e| panic!(e)),
        ))
        .1;

        trace!("response1: {:?}, response2: {:?}", response1, response2);

        assert!(response1.is_ok());
        assert_eq!(response1.ok().unwrap(), 123);

        assert!(response2.is_err());
        assert_eq!(response2.err().unwrap().kind(), io::ErrorKind::InvalidInput);
    }

    fn run_future<F>(f: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = futures::channel::oneshot::channel();
        tokio::run(
            f.map(|result| tx.send(result).unwrap_or_else(|_| unreachable!()))
                .boxed()
                .unit_error()
                .compat(),
        );
        futures::executor::block_on(rx).unwrap()
    }
}
