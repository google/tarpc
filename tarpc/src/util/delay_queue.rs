use std::{
    fmt::Debug,
    task::{Context, Poll},
    time::Duration,
};

pub use tokio_util::time::DelayQueue;

/// A trait that mocks [`DelayQueue`] with the minimal set of APIs that are needed for tarpc to run
/// This is needed so that user can supply their own implementation that satisfy the behavior of [`DelayQueue`]
/// So that the user can go runtime-agnostic on timer-related stuff (for example to run on smol or embassy)
pub trait DelayQueueLike<T>: Debug {
    /// The key returned from queue insertion, as a token to control the delay 
    type Key: Debug;
    /// Inserts `value` into the queue set to expire after the requested duration
    /// elapses.
    ///
    /// `value` is stored in the queue until `timeout` duration has
    /// elapsed after `insert` was called. At that point, `value` will
    /// be returned from [`poll_expired`]. If `timeout` is a `Duration` of
    /// zero, then `value` is immediately made available to poll.
    ///
    /// The return value represents the insertion and is used as an
    /// argument to [`remove`] and [`reset`]. Note that [`Key`] is a
    /// token and is reused once `value` is removed from the queue
    /// either by calling [`poll_expired`] after `timeout` has elapsed
    /// or by calling [`remove`]. At this point, the caller must not
    /// use the returned [`Key`] again as it may reference a different
    /// item in the queue.
    fn insert(&mut self, value: T, timeout: Duration) -> Self::Key;
    /// Removes the item associated with `key` from the queue.
    ///
    /// There must be an item associated with `key`. The function returns the
    /// removed item as well as the `Instant` at which it will the delay will
    /// have expired.
    fn remove(&mut self, key: &Self::Key);
    /// Clears the queue, removing all items.
    ///
    /// After calling `clear`, [`poll_expired`] will return `Ok(Ready(None))`.
    ///
    /// Note that this method has no effect on the allocated capacity.
    fn clear(&mut self);
    /// Attempts to pull out the next value of the delay queue, registering the
    /// current task for wakeup if the value is not yet available, and returning
    /// `None` if the queue is exhausted.
    fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>>;
    /// Returns `true` if there are no items in the queue.
    ///
    /// Note that this function returns `false` even if all items have not yet
    /// expired and a call to `poll` will return `Poll::Pending`.
    fn is_empty(&self) -> bool;
}

impl<T: Debug> DelayQueueLike<T> for DelayQueue<T> {
    type Key = tokio_util::time::delay_queue::Key;

    fn insert(&mut self, value: T, timeout: Duration) -> Self::Key {
        (self as &mut DelayQueue<T>).insert(value, timeout)
    }

    fn remove(&mut self, key: &Self::Key) {
        (self as &mut DelayQueue<T>).remove(key);
    }

    fn clear(&mut self) {
        (self as &mut DelayQueue<T>).clear();
    }

    fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        (self as &mut DelayQueue<T>)
            .poll_expired(cx)
            .map(|f| f.map(|x| x.into_inner()))
    }

    fn is_empty(&self) -> bool {
        (self as &DelayQueue<T>).is_empty()
    }
}
