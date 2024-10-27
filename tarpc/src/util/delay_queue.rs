use std::{
    fmt::Debug,
    task::{Context, Poll},
    time::Duration,
};

use tokio_util::time::delay_queue::Key;
pub use tokio_util::time::DelayQueue;

///
pub trait DelayQueueLike<T>: Debug {
    ///
    type Key: Debug;
    ///
    fn insert(&mut self, value: T, timeout: Duration) -> Self::Key;
    ///
    fn remove(&mut self, key: &Self::Key);
    ///
    fn clear(&mut self);
    ///
    fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>>;
    ///
    fn is_empty(&self) -> bool;
}

impl<T: Debug> DelayQueueLike<T> for DelayQueue<T> {
    type Key = Key;

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
