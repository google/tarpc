// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Transports backed by in-memory channels.

use crate::context::{ClientContext, ServerContext, SharedContext};
use crate::{ClientMessage, Response, Transport};
use futures::future::{Ready};
use futures::sink::With;
use futures::{Sink, SinkExt, Stream, TryStreamExt, task::*};
use pin_project::pin_project;
use std::{error::Error, future, pin::Pin};
use tokio::sync::mpsc;

/// Errors that occur in the sending or receiving of messages over a channel.
#[derive(thiserror::Error, Debug)]
pub enum ChannelError {
    /// An error occurred readying to send into the channel.
    #[error("an error occurred readying to send into the channel")]
    Ready(#[source] Box<dyn Error + Send + Sync + 'static>),
    /// An error occurred sending into the channel.
    #[error("an error occurred sending into the channel")]
    Send(#[source] Box<dyn Error + Send + Sync + 'static>),
    /// An error occurred receiving from the channel.
    #[error("an error occurred receiving from the channel")]
    Receive(#[source] Box<dyn Error + Send + Sync + 'static>),
}

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

/// Returns two mapped unbounded channel peers. Each [`Stream`] yields items sent through the other's
/// [`Sink`].
pub fn unbounded_mapped<
    SerializedSinkItem,
    SerializedItem,
    ClientSinkItem,
    ServerSinkItem,
    ClientItem,
    ServerItem,
    F,
    G,
    H,
    I,
>(
    mut f: F,
    mut g: G,
    mut h: H,
    mut i: I,
) -> (
    impl Transport<ClientSinkItem, ClientItem>,
    impl Transport<ServerItem, ServerSinkItem>,
)
where
    F: FnMut(ClientSinkItem) -> SerializedSinkItem,
    G: FnMut(SerializedSinkItem) -> ServerSinkItem,
    H: FnMut(SerializedItem) -> ClientItem,
    I: FnMut(ServerItem) -> SerializedItem,
{
    let (client, server) = unbounded();

    let client = client
        .with(move |msg: ClientSinkItem| future::ready(Ok(f(msg))))
        .map_ok(move |msg: SerializedItem| h(msg));
    let server = server
        .map_ok(move |msg: SerializedSinkItem| g(msg))
        .with(move |msg: ServerItem| future::ready(Ok(i(msg))));

    (client, server)
}

/// Convenience functino to return two mapped unbounded channel peers for a basechannel and a client implementation. Each [`Stream`] yields items sent through the other's
/// [`Sink`].
pub fn unbounded_for_client_server_context<Req, Resp>() -> (
    impl Transport<ClientMessage<ClientContext, Req>, Response<ClientContext, Resp>>,
    impl Transport<Response<ServerContext, Resp>, ClientMessage<ServerContext, Req>>,
) {
    unbounded_mapped(
        map_req_client_context_to_shared,
        map_req_shared_context_to_server,
        map_resp_shared_context_to_client,
        map_resp_server_context_to_shared,
    )
}

/// Convenience function to map a ClientMessage with ClientContext to one with SharedContext.
fn map_req_client_context_to_shared<Req>(
    msg: ClientMessage<ClientContext, Req>,
) -> ClientMessage<SharedContext, Req> {
    msg.map_context(|ctx| ctx.shared_context)
}

/// Convenience function to map a ClientMessage with SharedContext to one with ServerContext.
fn map_req_shared_context_to_server<Req>(
    msg: ClientMessage<SharedContext, Req>,
) -> ClientMessage<ServerContext, Req> {
    msg.map_context(ServerContext::new)
}

/// Convenience function to map a ClientMessage with ClientContext to one with SharedContext.
fn map_resp_server_context_to_shared<Req>(
    resp: Response<ServerContext, Req>,
) -> Response<SharedContext, Req> {
    resp.map_context(|ctx| ctx.shared_context)
}

/// Convenience function to map a ClientMessage with SharedContext to one with ServerContext.
fn map_resp_shared_context_to_client<Req>(
    msg: Response<SharedContext, Req>,
) -> Response<ClientContext, Req> {
    msg.map_context(ClientContext::new)
}

/// TODO: document
/// Yuck, but impl trait will loose our ability to do t.as_ref()
pub fn map_transport_to_client<Req, Resp, T, E>(
    t: T,
) -> futures::stream::MapOk<
    With<
        T,
        ClientMessage<SharedContext, Req>,
        ClientMessage<ClientContext, Req>,
        Ready<Result<ClientMessage<SharedContext, Req>, E>>,
        fn(ClientMessage<ClientContext, Req>) -> Ready<Result<ClientMessage<SharedContext, Req>, E>>,
    >,
    fn(Response<SharedContext, Resp>) -> Response<ClientContext, Resp>,
>
where
    T: Transport<ClientMessage<SharedContext, Req>, Response<SharedContext, Resp>>,
    E: From<T::TransportError>
{
    let f: fn(ClientMessage<ClientContext, Req>) -> Ready<Result<ClientMessage<SharedContext, Req>, E>> = |resp| futures::future::ok(map_req_client_context_to_shared(resp));

    t.with(f).map_ok(map_resp_shared_context_to_client)
}

/// TODO: document
///
/// Yuck, but impl trait will loose our ability to do t.as_ref()
pub fn map_transport_to_server<Req, Resp, T, E>(
    t: T,
) -> futures::stream::MapOk<
    With<
        T,
        Response<SharedContext, Resp>,
        Response<ServerContext, Resp>,
        Ready<Result<Response<SharedContext, Resp>, E>>,
        fn(Response<ServerContext, Resp>) -> Ready<Result<Response<SharedContext, Resp>, E>>,
    >,
    fn(ClientMessage<SharedContext, Req>) -> ClientMessage<ServerContext, Req>,
>
where
    T: Transport<Response<SharedContext, Resp>, ClientMessage<SharedContext, Req>>,
    E: From<T::TransportError>
{
    let f: fn(Response<ServerContext, Resp>) -> Ready<Result<Response<SharedContext, Resp>, E>> = |resp| futures::future::ok(map_resp_server_context_to_shared(resp));

    t.with(f)
        .map_ok(map_req_shared_context_to_server)
}

/// A bi-directional channel backed by an [`UnboundedSender`](mpsc::UnboundedSender)
/// and [`UnboundedReceiver`](mpsc::UnboundedReceiver).
#[derive(Debug)]
pub struct UnboundedChannel<Item, SinkItem> {
    rx: mpsc::UnboundedReceiver<Item>,
    tx: mpsc::UnboundedSender<SinkItem>,
}

impl<Item, SinkItem> Stream for UnboundedChannel<Item, SinkItem> {
    type Item = Result<Item, ChannelError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, ChannelError>>> {
        self.rx
            .poll_recv(cx)
            .map(|option| option.map(Ok))
            .map_err(ChannelError::Receive)
    }
}

const CLOSED_MESSAGE: &str = "the channel is closed and cannot accept new items for sending";

impl<Item, SinkItem> Sink<SinkItem> for UnboundedChannel<Item, SinkItem> {
    type Error = ChannelError;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(if self.tx.is_closed() {
            Err(ChannelError::Ready(CLOSED_MESSAGE.into()))
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
pub fn bounded<SinkItem, Item>(
    capacity: usize,
) -> (Channel<SinkItem, Item>, Channel<Item, SinkItem>) {
    let (tx1, rx2) = futures::channel::mpsc::channel(capacity);
    let (tx2, rx1) = futures::channel::mpsc::channel(capacity);
    (Channel { tx: tx1, rx: rx1 }, Channel { tx: tx2, rx: rx2 })
}

/// A bi-directional channel backed by a [`Sender`](futures::channel::mpsc::Sender)
/// and [`Receiver`](futures::channel::mpsc::Receiver).
#[pin_project]
#[derive(Debug)]
pub struct Channel<Item, SinkItem> {
    #[pin]
    rx: futures::channel::mpsc::Receiver<Item>,
    #[pin]
    tx: futures::channel::mpsc::Sender<SinkItem>,
}

impl<Item, SinkItem> Stream for Channel<Item, SinkItem> {
    type Item = Result<Item, ChannelError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, ChannelError>>> {
        self.project()
            .rx
            .poll_next(cx)
            .map(|option| option.map(Ok))
            .map_err(ChannelError::Receive)
    }
}

impl<Item, SinkItem> Sink<SinkItem> for Channel<Item, SinkItem> {
    type Error = ChannelError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .tx
            .poll_ready(cx)
            .map_err(|e| ChannelError::Ready(Box::new(e)))
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> Result<(), Self::Error> {
        self.project()
            .tx
            .start_send(item)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .tx
            .poll_flush(cx)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .tx
            .poll_close(cx)
            .map_err(|e| ChannelError::Send(Box::new(e)))
    }
}

#[cfg(all(test, feature = "tokio1"))]
mod tests {
    use crate::{
        ServerError,
        client::{self, RpcError},
        context,
        server::{BaseChannel, incoming::Incoming, serve},
        transport::{
            self,
            channel::{Channel, UnboundedChannel},
        },
    };
    use assert_matches::assert_matches;
    use futures::{prelude::*, stream};
    use std::io;
    use tracing::trace;

    #[test]
    fn ensure_is_transport() {
        fn is_transport<SinkItem, Item, T: crate::Transport<SinkItem, Item>>() {}
        is_transport::<(), (), UnboundedChannel<(), ()>>();
        is_transport::<(), (), Channel<(), ()>>();
    }

    #[tokio::test]
    async fn integration() -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        let (client_channel, server_channel) =
            transport::channel::unbounded_for_client_server_context();

        tokio::spawn(
            stream::once(future::ready(server_channel))
                .map(BaseChannel::with_defaults)
                .execute(serve(|_ctx, request: String| {
                    async move {
                        request.parse::<u64>().map_err(|_| {
                            ServerError::new(
                                io::ErrorKind::InvalidInput,
                                format!("{request:?} is not an int"),
                            )
                        })
                    }
                    .boxed()
                }))
                .for_each(|channel| async move {
                    tokio::spawn(channel.for_each(|response| response));
                }),
        );

        let client = client::new(client::Config::default(), client_channel).spawn();

        let response1 = client
            .call(&mut context::ClientContext::current(), "123".into())
            .await;
        let response2 = client
            .call(&mut context::ClientContext::current(), "abc".into())
            .await;

        trace!("response1: {:?}, response2: {:?}", response1, response2);

        assert_matches!(response1, Ok(123));
        assert_matches!(response2, Err(RpcError::Server(e)) if e.kind == io::ErrorKind::InvalidInput);

        Ok(())
    }
}
