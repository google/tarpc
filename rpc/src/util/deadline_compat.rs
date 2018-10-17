// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use futures::{
    compat::{Compat01As03, Future01CompatExt},
    prelude::*,
    ready, task::{Poll, LocalWaker},
};
use pin_utils::unsafe_pinned;
use std::pin::Pin;
use std::time::Instant;
use tokio_timer::{timeout, Delay};

#[must_use = "futures do nothing unless polled"]
#[derive(Debug)]
pub struct Deadline<T> {
    future: T,
    delay: Compat01As03<Delay>,
}

impl<T> Deadline<T> {
    unsafe_pinned!(future: T);
    unsafe_pinned!(delay: Compat01As03<Delay>);

    /// Create a new `Deadline` that completes when `future` completes or when
    /// `deadline` is reached.
    pub fn new(future: T, deadline: Instant) -> Deadline<T> {
        Deadline::new_with_delay(future, Delay::new(deadline))
    }

    pub(crate) fn new_with_delay(future: T, delay: Delay) -> Deadline<T> {
        Deadline {
            future,
            delay: delay.compat(),
        }
    }

    /// Gets a mutable reference to the underlying future in this deadline.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.future
    }
}
impl<T> Future for Deadline<T>
where
    T: TryFuture,
{
    type Output = Result<T::Ok, timeout::Error<T::Error>>;

    fn poll(mut self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Self::Output> {

        // First, try polling the future
        match self.future().try_poll(waker) {
            Poll::Ready(Ok(v)) => return Poll::Ready(Ok(v)),
            Poll::Pending => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(timeout::Error::inner(e))),
        }

        let delay = self.delay().poll_unpin(waker);

        // Now check the timer
        match ready!(delay) {
            Ok(_) => Poll::Ready(Err(timeout::Error::elapsed())),
            Err(e) => Poll::Ready(Err(timeout::Error::timer(e))),
        }
    }
}
