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
    use crate::context;
    use crate::client::{self, Client};
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

    #[test]
    fn reconnect() {
        use super::UnboundedChannel;
        use crate::transport::reconnecting;
        use std::pin::PinBox;

        let _ = env_logger::try_init();

        run_future(
            async {
                let (tx, mut rx) =
                    futures::channel::mpsc::unbounded::<UnboundedChannel<u64, u64>>();
                spawn!(
                    async move {
                        let mut server_channel = await!(rx.next()).unwrap();
                        trace!("Sending 1");
                        await!(server_channel.send(1)).unwrap();
                        trace!("Sent 1");
                        drop(server_channel);
                        let mut server_channel = await!(rx.next()).unwrap();
                        await!(server_channel.send(2)).unwrap();
                    }
                );
                trace!("Setting up reconnection");
                let mut client_channel = PinBox::new(reconnecting(
                    stream::repeat(move || {
                        let (client_channel, server_channel) = super::unbounded::<u64, u64>();
                        tx.unbounded_send(server_channel).unwrap();
                        futures::future::ready(Ok(client_channel))
                    }).then(|f| f()),
                ));
                assert_eq!(await!(client_channel.next()).unwrap().unwrap(), 1);
                let next = await!(client_channel.next()).unwrap();
                info!("{:?}", next);
                assert!(next.is_err());
                assert_eq!(next.err().unwrap().kind(), io::ErrorKind::BrokenPipe);
                assert_eq!(await!(client_channel.next()).unwrap().unwrap(), 2);
            },
        );
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
