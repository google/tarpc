// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! A TCP [`Transport`] that serializes as bincode.

#![feature(
    futures_api,
    pin,
    arbitrary_self_types,
    underscore_imports,
    await_macro,
    async_await,
    existential_type,
)]
#![deny(missing_docs, missing_debug_implementations)]

#[macro_use]
extern crate futures;
#[macro_use]
extern crate pin_utils;

mod vendored;

use crate::vendored::tokio_serde_bincode::{IoErrorWrapper, ReadBincode, WriteBincode};
use futures::{
    compat::{Compat, Future01CompatExt, Stream01CompatExt},
    prelude::*,
    task,
};
use futures_legacy::{
    executor::{
        self as executor01, Notify as Notify01, NotifyHandle as NotifyHandle01,
        UnsafeNotify as UnsafeNotify01,
    },
    sink::SinkMapErr as SinkMapErr01,
    stream::MapErr as MapErr01,
    Async as Async01, AsyncSink as AsyncSink01, Sink as Sink01, Stream as Stream01,
};
use serde::{Deserialize, Serialize};
use std::{fmt, io, marker::PhantomData, net::SocketAddr, pin::PinMut};
use tokio_io::codec::length_delimited;
use tokio_tcp::{self, TcpListener, TcpStream};

/// Returns a new bincode transport that reads from and writes to `io`.
pub fn new<Item, SinkItem>(io: TcpStream) -> Transport<Item, SinkItem>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    let peer_addr = io.peer_addr();
    let local_addr = io.local_addr();
    let inner = ReadBincode::new(WriteBincode::new(
        length_delimited::Builder::new()
            .max_frame_length(8_000_000)
            .new_framed(io)
            .map_err(IoErrorWrapper as _)
            .sink_map_err(IoErrorWrapper as _),
    ));

    Transport {
        inner,
        staged_item: None,
        peer_addr,
        local_addr,
    }
}

/// Connects to `addr`, wrapping the connection in a bincode transport.
pub async fn connect<Item, SinkItem>(addr: &SocketAddr) -> io::Result<Transport<Item, SinkItem>>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    let stream = await!(TcpStream::connect(addr).compat())?;
    Ok(new(stream))
}

/// Listens on `addr`, wrapping accepted connections in bincode transports.
pub fn listen<Item, SinkItem>(addr: &SocketAddr) -> io::Result<Incoming<Item, SinkItem>>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    let incoming = listener.incoming().compat();
    Ok(Incoming {
        incoming,
        local_addr,
        ghost: PhantomData,
    })
}

/// A [`TcpListener`] that wraps connections in bincode transports.
#[derive(Debug)]
pub struct Incoming<Item, SinkItem> {
    incoming: Compat<tokio_tcp::Incoming, ()>,
    local_addr: SocketAddr,
    ghost: PhantomData<(Item, SinkItem)>,
}

impl<Item, SinkItem> Incoming<Item, SinkItem> {
    unsafe_pinned!(incoming: Compat<tokio_tcp::Incoming, ()>);

    /// Returns the address being listened on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl<Item, SinkItem> Stream for Incoming<Item, SinkItem>
where
    Item: for<'a> Deserialize<'a>,
    SinkItem: Serialize,
{
    type Item = io::Result<Transport<Item, SinkItem>>;

    fn poll_next(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<Option<Self::Item>> {
        let next = ready!(self.incoming().poll_next(cx)?);
        Poll::Ready(next.map(|conn| Ok(new(conn))))
    }
}

/// A transport that serializes to, and deserializes from, a [`TcpStream`].
pub struct Transport<Item, SinkItem> {
    inner: ReadBincode<
        WriteBincode<
            SinkMapErr01<
                MapErr01<
                    length_delimited::Framed<tokio_tcp::TcpStream>,
                    fn(std::io::Error) -> IoErrorWrapper,
                >,
                fn(std::io::Error) -> IoErrorWrapper,
            >,
            SinkItem,
        >,
        Item,
    >,
    staged_item: Option<SinkItem>,
    peer_addr: io::Result<SocketAddr>,
    local_addr: io::Result<SocketAddr>,
}

impl<Item, SinkItem> fmt::Debug for Transport<Item, SinkItem> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Transport")
    }
}

impl<Item, SinkItem> Stream for Transport<Item, SinkItem>
where
    Item: for<'a> Deserialize<'a>,
{
    type Item = io::Result<Item>;

    fn poll_next(self: PinMut<Self>, cx: &mut task::Context) -> Poll<Option<io::Result<Item>>> {
        unsafe {
            let inner = &mut PinMut::get_mut_unchecked(self).inner;
            let mut compat = inner.compat();
            let compat = PinMut::new_unchecked(&mut compat);
            match ready!(compat.poll_next(cx)) {
                None => Poll::Ready(None),
                Some(Ok(next)) => Poll::Ready(Some(Ok(next))),
                Some(Err(e)) => Poll::Ready(Some(Err(e.0))),
            }
        }
    }
}

impl<Item, SinkItem> Sink for Transport<Item, SinkItem>
where
    SinkItem: Serialize,
{
    type SinkItem = SinkItem;
    type SinkError = io::Error;

    fn start_send(self: PinMut<Self>, item: SinkItem) -> io::Result<()> {
        let me = unsafe { PinMut::get_mut_unchecked(self) };
        assert!(me.staged_item.is_none());
        me.staged_item = Some(item);
        Ok(())
    }

    fn poll_ready(self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        let notify = &WakerToHandle(cx.waker());

        executor01::with_notify(notify, 0, move || {
            let me = unsafe { PinMut::get_mut_unchecked(self) };
            match me.staged_item.take() {
                Some(staged_item) => match me.inner.start_send(staged_item)? {
                    AsyncSink01::Ready => Poll::Ready(Ok(())),
                    AsyncSink01::NotReady(item) => {
                        me.staged_item = Some(item);
                        Poll::Pending
                    }
                },
                None => Poll::Ready(Ok(())),
            }
        })
    }

    fn poll_flush(self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        let notify = &WakerToHandle(cx.waker());

        executor01::with_notify(notify, 0, move || {
            let me = unsafe { PinMut::get_mut_unchecked(self) };
            match me.inner.poll_complete()? {
                Async01::Ready(()) => Poll::Ready(Ok(())),
                Async01::NotReady => Poll::Pending,
            }
        })
    }

    fn poll_close(self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        let notify = &WakerToHandle(cx.waker());

        executor01::with_notify(notify, 0, move || {
            let me = unsafe { PinMut::get_mut_unchecked(self) };
            match me.inner.get_mut().close()? {
                Async01::Ready(()) => Poll::Ready(Ok(())),
                Async01::NotReady => Poll::Pending,
            }
        })
    }
}

impl<Item, SinkItem> rpc::Transport for Transport<Item, SinkItem>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    type Item = Item;
    type SinkItem = SinkItem;

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        // TODO: should just access from the inner transport.
        // https://github.com/alexcrichton/tokio-serde-bincode/issues/4
        Ok(*self.peer_addr.as_ref().unwrap())
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(*self.local_addr.as_ref().unwrap())
    }
}

#[derive(Clone, Debug)]
struct WakerToHandle<'a>(&'a task::Waker);

#[derive(Debug)]
struct NotifyWaker(task::Waker);

impl Notify01 for NotifyWaker {
    fn notify(&self, _: usize) {
        self.0.wake();
    }
}

unsafe impl UnsafeNotify01 for NotifyWaker {
    unsafe fn clone_raw(&self) -> NotifyHandle01 {
        WakerToHandle(&self.0).into()
    }

    unsafe fn drop_raw(&self) {
        let ptr: *const dyn UnsafeNotify01 = self;
        drop(Box::from_raw(ptr as *mut dyn UnsafeNotify01));
    }
}

impl<'a> From<WakerToHandle<'a>> for NotifyHandle01 {
    fn from(handle: WakerToHandle<'a>) -> NotifyHandle01 {
        let ptr = Box::new(NotifyWaker(handle.0.clone()));

        unsafe { NotifyHandle01::new(Box::into_raw(ptr)) }
    }
}
