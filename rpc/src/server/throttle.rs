use super::{Channel, Config};
use crate::{Response, ServerError};
use futures::{
    future::AbortRegistration,
    prelude::*,
    ready,
    task::{Context, Poll},
};
use log::debug;
use pin_utils::{unsafe_pinned, unsafe_unpinned};
use std::{io, pin::Pin};

/// A [`Channel`] that limits the number of concurrent
/// requests by throttling.
#[derive(Debug)]
pub struct Throttler<C> {
    max_in_flight_requests: usize,
    inner: C,
}

impl<C> Throttler<C> {
    unsafe_unpinned!(max_in_flight_requests: usize);
    unsafe_pinned!(inner: C);

    /// Returns the inner channel.
    pub fn get_ref(&self) -> &C {
        &self.inner
    }
}

impl<C> Throttler<C>
where
    C: Channel,
{
    /// Returns a new `Throttler` that wraps the given channel and limits concurrent requests to
    /// `max_in_flight_requests`.
    pub fn new(inner: C, max_in_flight_requests: usize) -> Self {
        Throttler {
            inner,
            max_in_flight_requests,
        }
    }
}

impl<C> Stream for Throttler<C>
where
    C: Channel,
{
    type Item = <C as Stream>::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        while self.as_mut().in_flight_requests() >= *self.as_mut().max_in_flight_requests() {
            ready!(self.as_mut().inner().poll_ready(cx)?);

            match ready!(self.as_mut().inner().poll_next(cx)?) {
                Some(request) => {
                    debug!(
                        "[{}] Client has reached in-flight request limit ({}/{}).",
                        request.context.trace_id(),
                        self.as_mut().in_flight_requests(),
                        self.as_mut().max_in_flight_requests(),
                    );

                    self.as_mut().start_send(Response {
                        request_id: request.id,
                        message: Err(ServerError {
                            kind: io::ErrorKind::WouldBlock,
                            detail: Some("Server throttled the request.".into()),
                        }),
                    })?;
                }
                None => return Poll::Ready(None),
            }
        }
        self.inner().poll_next(cx)
    }
}

impl<C> Sink<Response<<C as Channel>::Resp>> for Throttler<C>
where
    C: Channel,
{
    type SinkError = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.inner().poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Response<<C as Channel>::Resp>) -> io::Result<()> {
        self.inner().start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.inner().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.inner().poll_close(cx)
    }
}

impl<C> AsRef<C> for Throttler<C> {
    fn as_ref(&self) -> &C {
        &self.inner
    }
}

impl<C> Channel for Throttler<C>
where
    C: Channel,
{
    type Req = <C as Channel>::Req;
    type Resp = <C as Channel>::Resp;

    fn in_flight_requests(self: Pin<&mut Self>) -> usize {
        self.inner().in_flight_requests()
    }

    fn config(&self) -> &Config {
        self.inner.config()
    }

    fn start_request(self: Pin<&mut Self>, request_id: u64) -> AbortRegistration {
        self.inner().start_request(request_id)
    }
}

/// A stream of throttling channels.
#[derive(Debug)]
pub struct ThrottlerStream<S> {
    inner: S,
    max_in_flight_requests: usize,
}

impl<S> ThrottlerStream<S>
where
    S: Stream,
    <S as Stream>::Item: Channel,
{
    unsafe_pinned!(inner: S);
    unsafe_unpinned!(max_in_flight_requests: usize);

    pub(crate) fn new(inner: S, max_in_flight_requests: usize) -> Self {
        Self {
            inner,
            max_in_flight_requests,
        }
    }
}

impl<S> Stream for ThrottlerStream<S>
where
    S: Stream,
    <S as Stream>::Item: Channel,
{
    type Item = Throttler<<S as Stream>::Item>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        match ready!(self.as_mut().inner().poll_next(cx)) {
            Some(channel) => Poll::Ready(Some(Throttler::new(
                channel,
                *self.max_in_flight_requests(),
            ))),
            None => Poll::Ready(None),
        }
    }
}
