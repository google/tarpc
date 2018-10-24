use futures::{compat::Stream01CompatExt, prelude::*, ready};
use futures_legacy::{
    executor::{
        self as executor01, Notify as Notify01, NotifyHandle as NotifyHandle01,
        UnsafeNotify as UnsafeNotify01,
    },
    Async as Async01, AsyncSink as AsyncSink01, Sink as Sink01, Stream as Stream01,
};
use std::{
    pin::Pin,
    task::{self, LocalWaker, Poll},
};

/// A shim to convert a 0.1 Sink + Stream to a 0.3 Sink + Stream.
#[derive(Debug)]
pub struct Compat<S, SinkItem> {
    staged_item: Option<SinkItem>,
    inner: S,
}

impl<S, SinkItem> Compat<S, SinkItem> {
    /// Returns a new Compat.
    pub fn new(inner: S) -> Self {
        Compat {
            inner,
            staged_item: None,
        }
    }

    /// Unwraps Compat, returning the inner value.
    pub fn into_inner(self) -> S {
        self.inner
    }

    /// Returns a reference to the value wrapped by Compat.
    pub fn get_ref(&self) -> &S {
        &self.inner
    }
}

impl<S, SinkItem> Stream for Compat<S, SinkItem>
where
    S: Stream01,
{
    type Item = Result<S::Item, S::Error>;

    fn poll_next(self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Option<Self::Item>> {
        unsafe {
            let inner = &mut Pin::get_mut_unchecked(self).inner;
            let mut compat = inner.compat();
            let compat = Pin::new_unchecked(&mut compat);
            match ready!(compat.poll_next(waker)) {
                None => Poll::Ready(None),
                Some(Ok(next)) => Poll::Ready(Some(Ok(next))),
                Some(Err(e)) => Poll::Ready(Some(Err(e))),
            }
        }
    }
}

impl<S, SinkItem> Sink for Compat<S, SinkItem>
where
    S: Sink01<SinkItem = SinkItem>,
{
    type SinkItem = SinkItem;
    type SinkError = S::SinkError;

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> Result<(), S::SinkError> {
        let me = unsafe { Pin::get_mut_unchecked(self) };
        assert!(me.staged_item.is_none());
        me.staged_item = Some(item);
        Ok(())
    }

    fn poll_ready(self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Result<(), S::SinkError>> {
        let notify = &WakerToHandle(waker);

        executor01::with_notify(notify, 0, move || {
            let me = unsafe { Pin::get_mut_unchecked(self) };
            match me.staged_item.take() {
                Some(staged_item) => match me.inner.start_send(staged_item) {
                    Ok(AsyncSink01::Ready) => Poll::Ready(Ok(())),
                    Ok(AsyncSink01::NotReady(item)) => {
                        me.staged_item = Some(item);
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                },
                None => Poll::Ready(Ok(())),
            }
        })
    }

    fn poll_flush(self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Result<(), S::SinkError>> {
        let notify = &WakerToHandle(waker);

        executor01::with_notify(notify, 0, move || {
            let me = unsafe { Pin::get_mut_unchecked(self) };
            match me.inner.poll_complete() {
                Ok(Async01::Ready(())) => Poll::Ready(Ok(())),
                Ok(Async01::NotReady) => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            }
        })
    }

    fn poll_close(self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Result<(), S::SinkError>> {
        let notify = &WakerToHandle(waker);

        executor01::with_notify(notify, 0, move || {
            let me = unsafe { Pin::get_mut_unchecked(self) };
            match me.inner.close() {
                Ok(Async01::Ready(())) => Poll::Ready(Ok(())),
                Ok(Async01::NotReady) => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            }
        })
    }
}

#[derive(Clone, Debug)]
struct WakerToHandle<'a>(&'a LocalWaker);

#[derive(Debug)]
struct NotifyWaker(task::Waker);

impl Notify01 for NotifyWaker {
    fn notify(&self, _: usize) {
        self.0.wake();
    }
}

unsafe impl UnsafeNotify01 for NotifyWaker {
    unsafe fn clone_raw(&self) -> NotifyHandle01 {
        let ptr = Box::new(NotifyWaker(self.0.clone()));

        NotifyHandle01::new(Box::into_raw(ptr))
    }

    unsafe fn drop_raw(&self) {
        let ptr: *const dyn UnsafeNotify01 = self;
        drop(Box::from_raw(ptr as *mut dyn UnsafeNotify01));
    }
}

impl<'a> From<WakerToHandle<'a>> for NotifyHandle01 {
    fn from(handle: WakerToHandle<'a>) -> NotifyHandle01 {
        unsafe { NotifyWaker(handle.0.clone().into_waker()).clone_raw() }
    }
}
