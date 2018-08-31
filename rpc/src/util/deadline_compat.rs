use futures::{
    compat::{Compat, Future01CompatExt},
    prelude::*,
    task,
};
use pin_utils::unsafe_pinned;
use std::pin::PinMut;
use std::time::Instant;
use tokio_timer::{timeout, Delay};

#[must_use = "futures do nothing unless polled"]
#[derive(Debug)]
pub struct Deadline<T> {
    future: T,
    delay: Compat<Delay, ()>,
}

impl<T> Deadline<T> {
    unsafe_pinned!(future: T);
    unsafe_pinned!(delay: Compat<Delay, ()>);

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

    fn poll(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        // First, try polling the future
        match self.future().try_poll(cx) {
            Poll::Ready(Ok(v)) => return Poll::Ready(Ok(v)),
            Poll::Pending => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(timeout::Error::inner(e))),
        }

        // Now check the timer
        match ready!(self.delay().poll_unpin(cx)) {
            Ok(_) => Poll::Ready(Err(timeout::Error::elapsed())),
            Err(e) => Poll::Ready(Err(timeout::Error::timer(e))),
        }
    }
}
