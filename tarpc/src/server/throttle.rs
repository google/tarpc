// Copyright 2020 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use super::{Channel, Config};
use crate::{Response, ServerError};
use futures::{future::AbortRegistration, prelude::*, ready, task::*};
use pin_project::pin_project;
use std::{io, pin::Pin, time::SystemTime};
use tracing::Span;

/// A [`Channel`] that limits the number of concurrent
/// requests by throttling.
#[pin_project]
#[derive(Debug)]
pub struct Throttler<C> {
    max_in_flight_requests: usize,
    #[pin]
    inner: C,
}

impl<C> Throttler<C> {
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
            max_in_flight_requests,
            inner,
        }
    }
}

impl<C> Stream for Throttler<C>
where
    C: Channel,
{
    type Item = <C as Stream>::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        while self.as_mut().in_flight_requests() >= *self.as_mut().project().max_in_flight_requests
        {
            ready!(self.as_mut().project().inner.poll_ready(cx)?);

            match ready!(self.as_mut().project().inner.poll_next(cx)?) {
                Some(request) => {
                    tracing::debug!(
                        rpc.trace_id = %request.context.trace_id(),
                        in_flight_requests = self.as_mut().in_flight_requests(),
                        max_in_flight_requests = *self.as_mut().project().max_in_flight_requests,
                        "At in-flight request limit",
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
        self.project().inner.poll_next(cx)
    }
}

impl<C> Sink<Response<<C as Channel>::Resp>> for Throttler<C>
where
    C: Channel,
{
    type Error = C::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx)
    }

    fn start_send(
        self: Pin<&mut Self>,
        item: Response<<C as Channel>::Resp>,
    ) -> Result<(), Self::Error> {
        self.project().inner.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx)
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
    type Transport = <C as Channel>::Transport;

    fn in_flight_requests(&self) -> usize {
        self.inner.in_flight_requests()
    }

    fn config(&self) -> &Config {
        self.inner.config()
    }

    fn transport(&self) -> &Self::Transport {
        self.inner.transport()
    }

    fn start_request(
        self: Pin<&mut Self>,
        id: u64,
        deadline: SystemTime,
        span: Span,
    ) -> Result<AbortRegistration, super::in_flight_requests::AlreadyExistsError> {
        self.project().inner.start_request(id, deadline, span)
    }
}

/// A stream of throttling channels.
#[pin_project]
#[derive(Debug)]
pub struct ThrottlerStream<S> {
    #[pin]
    inner: S,
    max_in_flight_requests: usize,
}

impl<S> ThrottlerStream<S>
where
    S: Stream,
    <S as Stream>::Item: Channel,
{
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
        match ready!(self.as_mut().project().inner.poll_next(cx)) {
            Some(channel) => Poll::Ready(Some(Throttler::new(
                channel,
                *self.project().max_in_flight_requests,
            ))),
            None => Poll::Ready(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        server::{
            in_flight_requests::AlreadyExistsError,
            testing::{self, FakeChannel, PollExt},
        },
        Request,
    };
    use pin_utils::pin_mut;
    use std::{marker::PhantomData, time::Duration};

    #[tokio::test]
    async fn throttler_in_flight_requests() {
        let throttler = Throttler {
            max_in_flight_requests: 0,
            inner: FakeChannel::default::<isize, isize>(),
        };

        pin_mut!(throttler);
        for i in 0..5 {
            throttler
                .inner
                .in_flight_requests
                .start_request(
                    i,
                    SystemTime::now() + Duration::from_secs(1),
                    Span::current(),
                )
                .unwrap();
        }
        assert_eq!(throttler.as_mut().in_flight_requests(), 5);
    }

    #[tokio::test]
    async fn throttler_start_request() {
        let throttler = Throttler {
            max_in_flight_requests: 0,
            inner: FakeChannel::default::<isize, isize>(),
        };

        pin_mut!(throttler);
        throttler
            .as_mut()
            .start_request(
                1,
                SystemTime::now() + Duration::from_secs(1),
                Span::current(),
            )
            .unwrap();
        assert_eq!(throttler.inner.in_flight_requests.len(), 1);
    }

    #[test]
    fn throttler_poll_next_done() {
        let throttler = Throttler {
            max_in_flight_requests: 0,
            inner: FakeChannel::default::<isize, isize>(),
        };

        pin_mut!(throttler);
        assert!(throttler.as_mut().poll_next(&mut testing::cx()).is_done());
    }

    #[test]
    fn throttler_poll_next_some() -> io::Result<()> {
        let throttler = Throttler {
            max_in_flight_requests: 1,
            inner: FakeChannel::default::<isize, isize>(),
        };

        pin_mut!(throttler);
        throttler.inner.push_req(0, 1);
        assert!(throttler.as_mut().poll_ready(&mut testing::cx()).is_ready());
        assert_eq!(
            throttler
                .as_mut()
                .poll_next(&mut testing::cx())?
                .map(|r| r.map(|r| (r.id, r.message))),
            Poll::Ready(Some((0, 1)))
        );
        Ok(())
    }

    #[test]
    fn throttler_poll_next_throttled() {
        let throttler = Throttler {
            max_in_flight_requests: 0,
            inner: FakeChannel::default::<isize, isize>(),
        };

        pin_mut!(throttler);
        throttler.inner.push_req(1, 1);
        assert!(throttler.as_mut().poll_next(&mut testing::cx()).is_done());
        assert_eq!(throttler.inner.sink.len(), 1);
        let resp = throttler.inner.sink.get(0).unwrap();
        assert_eq!(resp.request_id, 1);
        assert!(resp.message.is_err());
    }

    #[test]
    fn throttler_poll_next_throttled_sink_not_ready() {
        let throttler = Throttler {
            max_in_flight_requests: 0,
            inner: PendingSink::default::<isize, isize>(),
        };
        pin_mut!(throttler);
        assert!(throttler.poll_next(&mut testing::cx()).is_pending());

        struct PendingSink<In, Out> {
            ghost: PhantomData<fn(Out) -> In>,
        }
        impl PendingSink<(), ()> {
            pub fn default<Req, Resp>() -> PendingSink<io::Result<Request<Req>>, Response<Resp>> {
                PendingSink { ghost: PhantomData }
            }
        }
        impl<In, Out> Stream for PendingSink<In, Out> {
            type Item = In;
            fn poll_next(self: Pin<&mut Self>, _: &mut Context) -> Poll<Option<Self::Item>> {
                unimplemented!()
            }
        }
        impl<In, Out> Sink<Out> for PendingSink<In, Out> {
            type Error = io::Error;
            fn poll_ready(self: Pin<&mut Self>, _: &mut Context) -> Poll<Result<(), Self::Error>> {
                Poll::Pending
            }
            fn start_send(self: Pin<&mut Self>, _: Out) -> Result<(), Self::Error> {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            }
            fn poll_flush(self: Pin<&mut Self>, _: &mut Context) -> Poll<Result<(), Self::Error>> {
                Poll::Pending
            }
            fn poll_close(self: Pin<&mut Self>, _: &mut Context) -> Poll<Result<(), Self::Error>> {
                Poll::Pending
            }
        }
        impl<Req, Resp> Channel for PendingSink<io::Result<Request<Req>>, Response<Resp>> {
            type Req = Req;
            type Resp = Resp;
            type Transport = ();
            fn config(&self) -> &Config {
                unimplemented!()
            }
            fn in_flight_requests(&self) -> usize {
                0
            }
            fn transport(&self) -> &() {
                &()
            }
            fn start_request(
                self: Pin<&mut Self>,
                _id: u64,
                _deadline: SystemTime,
                _span: tracing::Span,
            ) -> Result<AbortRegistration, AlreadyExistsError> {
                unimplemented!()
            }
        }
    }

    #[tokio::test]
    async fn throttler_start_send() {
        let throttler = Throttler {
            max_in_flight_requests: 0,
            inner: FakeChannel::default::<isize, isize>(),
        };

        pin_mut!(throttler);
        throttler
            .inner
            .in_flight_requests
            .start_request(
                0,
                SystemTime::now() + Duration::from_secs(1),
                Span::current(),
            )
            .unwrap();
        throttler
            .as_mut()
            .start_send(Response {
                request_id: 0,
                message: Ok(1),
            })
            .unwrap();
        assert_eq!(throttler.inner.in_flight_requests.len(), 0);
        assert_eq!(
            throttler.inner.sink.get(0),
            Some(&Response {
                request_id: 0,
                message: Ok(1),
            })
        );
    }
}
