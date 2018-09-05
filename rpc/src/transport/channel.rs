//! Transports backed by in-memory channels.

use crate::Transport;
use futures::{channel::mpsc, task, Poll, Sink, Stream};
use pin_utils::unsafe_pinned;
use std::pin::PinMut;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

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

    fn poll_next(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<Option<io::Result<Item>>> {
        self.rx().poll_next(cx).map(|option| option.map(Ok))
    }
}

impl<Item, SinkItem> Sink for UnboundedChannel<Item, SinkItem> {
    type SinkItem = SinkItem;
    type SinkError = io::Error;

    fn poll_ready(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        self.tx()
            .poll_ready(cx)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn start_send(mut self: PinMut<Self>, item: SinkItem) -> io::Result<()> {
        self.tx()
            .start_send(item)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn poll_flush(
        mut self: PinMut<Self>,
        cx: &mut task::Context,
    ) -> Poll<Result<(), Self::SinkError>> {
        self.tx()
            .poll_flush(cx)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn poll_close(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        self.tx()
            .poll_close(cx)
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))
    }
}

impl<Item, SinkItem> Transport for UnboundedChannel<Item, SinkItem> {
    type Item = Item;
    type SinkItem = SinkItem;

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
    }
}

#[cfg(test)]
mod tests {
    use crate::client::{self, Client};
    use crate::context;
    use crate::server::{self, Handler, Server};
    use crate::transport;
    use env_logger;
    use futures::compat::TokioDefaultSpawner;
    use futures::future;
    use futures::{prelude::*, stream};
    use std::io;

    #[test]
    fn parse() {
        let _ = env_logger::try_init();

        let (client_channel, server_channel) = transport::channel::unbounded();
        let server = Server::<String, u64>::new(server::Config::default())
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
            let mut client = await!(Client::new(client::Config::default(), client_channel));

            let response1 = await!(client.send(context::current(), "123"));
            let response2 = await!(client.send(context::current(), "abc"));

            Ok::<_, io::Error>((response1, response2))
        };

        let (response1, response2) =
            run_future(server.join(responses.unwrap_or_else(|e| panic!(e)))).1;

        println!("response1: {:?}, response2: {:?}", response1, response2);

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
                .compat(TokioDefaultSpawner),
        );
        futures::executor::block_on(rx).unwrap()
    }
}
