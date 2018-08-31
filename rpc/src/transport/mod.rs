//! Provides a [`Transport`] trait as well as implementations.
//!
//! The rpc crate is transport- and protocol-agnostic. Any transport that impls [`Transport`]
//! can be plugged in, using whatever protocol it wants.

use crate::util::stream;
use futures::{compat::Future01CompatExt, prelude::*};
use std::future::get_task_cx;
use std::pin::PinBox;
use std::task::Poll;
use std::time::{Duration, Instant};
use std::{io, net::SocketAddr};
use tokio_timer::Delay;

pub mod channel;

/// A bidirectional stream ([`Sink`] + [`Stream`]) of messages.
pub trait Transport
where
    Self: Stream<Item = io::Result<<Self as Transport>::Item>>,
    Self: Sink<SinkItem = <Self as Transport>::SinkItem, SinkError = io::Error>,
{
    type Item;
    type SinkItem;

    /// The address of the remote peer this transport is in communication with.
    fn peer_addr(&self) -> io::Result<SocketAddr>;
    /// The address of the local half of this transport.
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

pub macro await_stream($e:expr) {
    loop {
        match get_task_cx($e) {
            Poll::Ready(t) => break t,
            Poll::Pending => yield Poll::Pending,
        }
    }
}

pub fn reconnecting<T>(
    transport_stream: impl Stream<Item = io::Result<T>>,
) -> impl Stream<Item = io::Result<<T as Transport>::Item>>
where
    T: Transport,
{
    stream::from_generator(move || {
        let mut transport_stream = PinBox::new(transport_stream);

        let mut i = 0;
        'start_over: loop {
            let millis = i * 100;
            info!("Connecting in {} millis", millis);
            let mut delay =
                PinBox::new(Delay::new(Instant::now() + Duration::from_millis(millis)).compat());
            await_stream!(|cx| delay.as_pin_mut().poll(cx)).unwrap();
            i += 1;

            // Resolve the transport.
            let transport = match await_stream!(|cx| transport_stream.as_pin_mut().poll_next(cx)) {
                None => return,
                Some(Ok(transport)) => transport,
                Some(Err(e)) => {
                    yield Poll::Ready(Err(e));
                    continue 'start_over;
                }
            };

            // Yield the next item from the stream.
            let mut transport = PinBox::new(transport);
            loop {
                match await_stream!(|cx| transport.as_pin_mut().poll_next(cx)) {
                    None => {
                        yield Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "transport closed",
                        )));
                        continue 'start_over;
                    }
                    Some(Ok(item)) => {
                        yield Poll::Ready(Ok(item));
                    }
                    Some(Err(e)) => {
                        yield Poll::Ready(Err(e));
                        continue 'start_over;
                    }
                }
            }
        }
    })
}
