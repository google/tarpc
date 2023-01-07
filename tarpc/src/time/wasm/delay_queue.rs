//! A queue of delayed elements.
//!
//! See [`DelayQueue`] for more details.
//!
//! [`DelayQueue`]: struct@DelayQueue

use super::wheel::{self, Wheel};

use super::{sleep_until, Instant, Sleep};
use futures::{ready, Stream};
use std::time::Duration;

use core::ops::{Index, IndexMut};
use slab::Slab;
use std::cmp;
use std::collections::HashMap;
use std::convert::From;
use std::fmt;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{self, Poll, Waker};

#[derive(Debug)]
pub struct DelayQueue<T> {
    slab: SlabStorage<T>,
    wheel: Wheel<Stack<T>>,
    expired: Stack<T>,
    delay: Option<Pin<Box<Sleep>>>,
    wheel_now: u64,
    start: Instant,
    waker: Option<Waker>,
}

#[derive(Default)]
struct SlabStorage<T> {
    inner: Slab<Data<T>>,
    key_map: HashMap<Key, KeyInternal>,
    next_key_index: usize,
    compact_called: bool,
}

impl<T> SlabStorage<T> {
    pub(crate) fn with_capacity(capacity: usize) -> SlabStorage<T> {
        SlabStorage {
            inner: Slab::with_capacity(capacity),
            key_map: HashMap::new(),
            next_key_index: 0,
            compact_called: false,
        }
    }

    // Inserts data into the inner slab and re-maps keys if necessary
    pub(crate) fn insert(&mut self, val: Data<T>) -> Key {
        let mut key = KeyInternal::new(self.inner.insert(val));
        let key_contained = self.key_map.contains_key(&key.into());

        if key_contained {
            // It's possible that a `compact` call creates capacity in `self.inner` in
            // such a way that a `self.inner.insert` call creates a `key` which was
            // previously given out during an `insert` call prior to the `compact` call.
            // If `key` is contained in `self.key_map`, we have encountered this exact situation,
            // We need to create a new key `key_to_give_out` and include the relation
            // `key_to_give_out` -> `key` in `self.key_map`.
            let key_to_give_out = self.create_new_key();
            assert!(!self.key_map.contains_key(&key_to_give_out.into()));
            self.key_map.insert(key_to_give_out.into(), key);
            key = key_to_give_out;
        } else if self.compact_called {
            // Include an identity mapping in `self.key_map` in order to allow us to
            // panic if a key that was handed out is removed more than once.
            self.key_map.insert(key.into(), key);
        }

        key.into()
    }

    // Re-map the key in case compact was previously called.
    // Note: Since we include identity mappings in key_map after compact was called,
    // we have information about all keys that were handed out. In the case in which
    // compact was called and we try to remove a Key that was previously removed
    // we can detect invalid keys if no key is found in `key_map`. This is necessary
    // in order to prevent situations in which a previously removed key
    // corresponds to a re-mapped key internally and which would then be incorrectly
    // removed from the slab.
    //
    // Example to illuminate this problem:
    //
    // Let's assume our `key_map` is {1 -> 2, 2 -> 1} and we call remove(1). If we
    // were to remove 1 again, we would not find it inside `key_map` anymore.
    // If we were to imply from this that no re-mapping was necessary, we would
    // incorrectly remove 1 from `self.slab.inner`, which corresponds to the
    // handed-out key 2.
    pub(crate) fn remove(&mut self, key: &Key) -> Data<T> {
        let remapped_key = if self.compact_called {
            match self.key_map.remove(key) {
                Some(key_internal) => key_internal,
                None => panic!("invalid key"),
            }
        } else {
            (*key).into()
        };

        self.inner.remove(remapped_key.index)
    }

    // Tries to re-map a `Key` that was given out to the user to its
    // corresponding internal key.
    fn remap_key(&self, key: &Key) -> Option<KeyInternal> {
        let key_map = &self.key_map;
        if self.compact_called {
            key_map.get(key).copied()
        } else {
            Some((*key).into())
        }
    }

    fn create_new_key(&mut self) -> KeyInternal {
        while self.key_map.contains_key(&Key::new(self.next_key_index)) {
            self.next_key_index = self.next_key_index.wrapping_add(1);
        }

        KeyInternal::new(self.next_key_index)
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    pub(crate) fn contains(&self, key: &Key) -> bool {
        let remapped_key = self.remap_key(key);

        match remapped_key {
            Some(internal_key) => self.inner.contains(internal_key.index),
            None => false,
        }
    }
}

impl<T> fmt::Debug for SlabStorage<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        if fmt.alternate() {
            fmt.debug_map().entries(self.inner.iter()).finish()
        } else {
            fmt.debug_struct("Slab")
                .field("len", &self.len())
                .field("cap", &self.capacity())
                .finish()
        }
    }
}

impl<T> Index<Key> for SlabStorage<T> {
    type Output = Data<T>;

    fn index(&self, key: Key) -> &Self::Output {
        let remapped_key = self.remap_key(&key);

        match remapped_key {
            Some(internal_key) => &self.inner[internal_key.index],
            None => panic!("Invalid index {}", key.index),
        }
    }
}

impl<T> IndexMut<Key> for SlabStorage<T> {
    fn index_mut(&mut self, key: Key) -> &mut Data<T> {
        let remapped_key = self.remap_key(&key);

        match remapped_key {
            Some(internal_key) => &mut self.inner[internal_key.index],
            None => panic!("Invalid index {}", key.index),
        }
    }
}

/// An entry in `DelayQueue` that has expired and been removed.
///
/// Values are returned by [`DelayQueue::poll_expired`].
///
/// [`DelayQueue::poll_expired`]: method@DelayQueue::poll_expired
#[derive(Debug)]
pub struct Expired<T> {
    /// The data stored in the queue
    data: T,
}

/// Token to a value stored in a `DelayQueue`.
///
/// Instances of `Key` are returned by [`DelayQueue::insert`]. See [`DelayQueue`]
/// documentation for more details.
///
/// [`DelayQueue`]: struct@DelayQueue
/// [`DelayQueue::insert`]: method@DelayQueue::insert
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    index: usize,
}

// Whereas `Key` is given out to users that use `DelayQueue`, internally we use
// `KeyInternal` as the key type in order to make the logic of mapping between keys
// as a result of `compact` calls clearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct KeyInternal {
    index: usize,
}

#[derive(Debug)]
struct Stack<T> {
    /// Head of the stack
    head: Option<Key>,
    _p: PhantomData<fn() -> T>,
}

#[derive(Debug)]
struct Data<T> {
    /// The data being stored in the queue and will be returned at the requested
    /// instant.
    inner: T,

    /// The instant at which the item is returned.
    when: u64,

    /// Set to true when stored in the `expired` queue
    expired: bool,

    /// Next entry in the stack
    next: Option<Key>,

    /// Previous entry in the stack
    prev: Option<Key>,
}

/// Maximum number of entries the queue can handle
const MAX_ENTRIES: usize = (1 << 30) - 1;

impl<T> DelayQueue<T> {
    /// Creates a new, empty, `DelayQueue`.
    ///
    /// The queue will not allocate storage until items are inserted into it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use tokio_util::time::DelayQueue;
    /// let delay_queue: DelayQueue<u32> = DelayQueue::new();
    /// ```
    pub fn new() -> DelayQueue<T> {
        DelayQueue::with_capacity(0)
    }

    /// Creates a new, empty, `DelayQueue` with the specified capacity.
    ///
    /// The queue will be able to hold at least `capacity` elements without
    /// reallocating. If `capacity` is 0, the queue will not allocate for
    /// storage.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use tokio_util::time::DelayQueue;
    /// # use std::time::Duration;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut delay_queue = DelayQueue::with_capacity(10);
    ///
    /// // These insertions are done without further allocation
    /// for i in 0..10 {
    ///     delay_queue.insert(i, Duration::from_secs(i));
    /// }
    ///
    /// // This will make the queue allocate additional storage
    /// delay_queue.insert(11, Duration::from_secs(11));
    /// # }
    /// ```
    pub fn with_capacity(capacity: usize) -> DelayQueue<T> {
        DelayQueue {
            wheel: Wheel::new(),
            slab: SlabStorage::with_capacity(capacity),
            expired: Stack::default(),
            delay: None,
            wheel_now: 0,
            start: Instant::now(),
            waker: None,
        }
    }

    /// Inserts `value` into the queue set to expire at a specific instant in
    /// time.
    ///
    /// This function is identical to `insert`, but takes an `Instant` instead
    /// of a `Duration`.
    ///
    /// `value` is stored in the queue until `when` is reached. At which point,
    /// `value` will be returned from [`poll_expired`]. If `when` has already been
    /// reached, then `value` is immediately made available to poll.
    ///
    /// The return value represents the insertion and is used as an argument to
    /// [`remove`] and [`reset`]. Note that [`Key`] is a token and is reused once
    /// `value` is removed from the queue either by calling [`poll_expired`] after
    /// `when` is reached or by calling [`remove`]. At this point, the caller
    /// must take care to not use the returned [`Key`] again as it may reference
    /// a different item in the queue.
    ///
    /// See [type] level documentation for more details.
    ///
    /// # Panics
    ///
    /// This function panics if `when` is too far in the future.
    ///
    /// # Examples
    ///
    /// Basic usage
    ///
    /// ```rust
    /// use tokio::time::{Duration, Instant};
    /// use tokio_util::time::DelayQueue;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut delay_queue = DelayQueue::new();
    /// let key = delay_queue.insert_at(
    ///     "foo", Instant::now() + Duration::from_secs(5));
    ///
    /// // Remove the entry
    /// let item = delay_queue.remove(&key);
    /// assert_eq!(*item.get_ref(), "foo");
    /// # }
    /// ```
    ///
    /// [`poll_expired`]: method@Self::poll_expired
    /// [`remove`]: method@Self::remove
    /// [`reset`]: method@Self::reset
    /// [`Key`]: struct@Key
    /// [type]: #
    pub fn insert_at(&mut self, value: T, when: Instant) -> Key {
        assert!(self.slab.len() < MAX_ENTRIES, "max entries exceeded");

        // Normalize the deadline. Values cannot be set to expire in the past.
        let when = self.normalize_deadline(when);

        // Insert the value in the store
        let key = self.slab.insert(Data {
            inner: value,
            when,
            expired: false,
            next: None,
            prev: None,
        });

        self.insert_idx(when, key);

        // Set a new delay if the current's deadline is later than the one of the new item
        let should_set_delay = if let Some(ref delay) = self.delay {
            let current_exp = self.normalize_deadline(delay.deadline());
            current_exp > when
        } else {
            true
        };

        if should_set_delay {
            if let Some(waker) = self.waker.take() {
                waker.wake();
            }

            let delay_time = self.start + Duration::from_millis(when);
            if let Some(ref mut delay) = &mut self.delay {
                delay.as_mut().reset(delay_time);
            } else {
                self.delay = Some(Box::pin(sleep_until(delay_time)));
            }
        }

        key
    }

    /// Attempts to pull out the next value of the delay queue, registering the
    /// current task for wakeup if the value is not yet available, and returning
    /// `None` if the queue is exhausted.
    pub fn poll_expired(&mut self, cx: &mut task::Context<'_>) -> Poll<Option<Expired<T>>> {
        if !self
            .waker
            .as_ref()
            .map(|w| w.will_wake(cx.waker()))
            .unwrap_or(false)
        {
            self.waker = Some(cx.waker().clone());
        }

        let item = ready!(self.poll_idx(cx));
        Poll::Ready(item.map(|key| {
            let data = self.slab.remove(&key);
            debug_assert!(data.next.is_none());
            debug_assert!(data.prev.is_none());

            Expired { data: data.inner }
        }))
    }

    /// Inserts `value` into the queue set to expire after the requested duration
    /// elapses.
    ///
    /// This function is identical to `insert_at`, but takes a `Duration`
    /// instead of an `Instant`.
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
    ///
    /// See [type] level documentation for more details.
    ///
    /// # Panics
    ///
    /// This function panics if `timeout` is greater than the maximum
    /// duration supported by the timer in the current `Runtime`.
    ///
    /// # Examples
    ///
    /// Basic usage
    ///
    /// ```rust
    /// use tokio_util::time::DelayQueue;
    /// use std::time::Duration;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut delay_queue = DelayQueue::new();
    /// let key = delay_queue.insert("foo", Duration::from_secs(5));
    ///
    /// // Remove the entry
    /// let item = delay_queue.remove(&key);
    /// assert_eq!(*item.get_ref(), "foo");
    /// # }
    /// ```
    ///
    /// [`poll_expired`]: method@Self::poll_expired
    /// [`remove`]: method@Self::remove
    /// [`reset`]: method@Self::reset
    /// [`Key`]: struct@Key
    /// [type]: #
    pub fn insert(&mut self, value: T, timeout: Duration) -> Key {
        self.insert_at(value, Instant::now() + timeout)
    }

    fn insert_idx(&mut self, when: u64, key: Key) {
        use self::wheel::{InsertError, Stack};

        // Register the deadline with the timer wheel
        match self.wheel.insert(when, key, &mut self.slab) {
            Ok(_) => {}
            Err((_, InsertError::Elapsed)) => {
                self.slab[key].expired = true;
                // The delay is already expired, store it in the expired queue
                self.expired.push(key, &mut self.slab);
            }
            Err((_, err)) => panic!("invalid deadline; err={:?}", err),
        }
    }

    /// Removes the key from the expired queue or the timer wheel
    /// depending on its expiration status.
    ///
    /// # Panics
    ///
    /// Panics if the key is not contained in the expired queue or the wheel.
    fn remove_key(&mut self, key: &Key) {
        use super::wheel::Stack;
        // Special case the `expired` queue
        if self.slab[*key].expired {
            self.expired.remove(key, &mut self.slab);
        } else {
            self.wheel.remove(key, &mut self.slab);
        }
    }

    /// Removes the item associated with `key` from the queue.
    ///
    /// There must be an item associated with `key`. The function returns the
    /// removed item as well as the `Instant` at which it will the delay will
    /// have expired.
    ///
    /// # Panics
    ///
    /// The function panics if `key` is not contained by the queue.
    ///
    /// # Examples
    ///
    /// Basic usage
    ///
    /// ```rust
    /// use tokio_util::time::DelayQueue;
    /// use std::time::Duration;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut delay_queue = DelayQueue::new();
    /// let key = delay_queue.insert("foo", Duration::from_secs(5));
    ///
    /// // Remove the entry
    /// let item = delay_queue.remove(&key);
    /// assert_eq!(*item.get_ref(), "foo");
    /// # }
    /// ```
    pub fn remove(&mut self, key: &Key) -> Expired<T> {
        let prev_deadline = self.next_deadline();

        self.remove_key(key);
        let data = self.slab.remove(key);

        let next_deadline = self.next_deadline();
        if prev_deadline != next_deadline {
            match (next_deadline, &mut self.delay) {
                (None, _) => self.delay = None,
                (Some(deadline), Some(delay)) => delay.as_mut().reset(deadline),
                (Some(deadline), None) => self.delay = Some(Box::pin(sleep_until(deadline))),
            }
        }

        Expired { data: data.inner }
    }

    /// Returns the next time to poll as determined by the wheel
    fn next_deadline(&mut self) -> Option<Instant> {
        self.wheel
            .poll_at()
            .map(|poll_at| self.start + Duration::from_millis(poll_at))
    }

    /// Polls the queue, returning the index of the next slot in the slab that
    /// should be returned.
    ///
    /// A slot should be returned when the associated deadline has been reached.
    fn poll_idx(&mut self, cx: &mut task::Context<'_>) -> Poll<Option<Key>> {
        use self::wheel::Stack;

        let expired = self.expired.pop(&mut self.slab);

        if expired.is_some() {
            return Poll::Ready(expired);
        }

        loop {
            if let Some(ref mut delay) = self.delay {
                #[allow(unused_must_use)]
                {
                    ready!(Pin::new(&mut *delay).poll(cx));
                }

                let now = super::ms(delay.deadline() - self.start, super::Round::Down);

                self.wheel_now = now;
            }

            // We poll the wheel to get the next value out before finding the next deadline.
            let wheel_idx = self.wheel.poll(self.wheel_now, &mut self.slab);

            self.delay = self.next_deadline().map(|when| Box::pin(sleep_until(when)));

            if let Some(idx) = wheel_idx {
                return Poll::Ready(Some(idx));
            }

            if self.delay.is_none() {
                return Poll::Ready(None);
            }
        }
    }

    fn normalize_deadline(&self, when: Instant) -> u64 {
        let when = if when < self.start {
            0
        } else {
            super::ms(when - self.start, super::Round::Up)
        };

        cmp::max(when, self.wheel.elapsed())
    }
}

// We never put `T` in a `Pin`...
impl<T> Unpin for DelayQueue<T> {}

impl<T> Default for DelayQueue<T> {
    fn default() -> DelayQueue<T> {
        DelayQueue::new()
    }
}

impl<T> Stream for DelayQueue<T> {
    // DelayQueue seems much more specific, where a user may care that it
    // has reached capacity, so return those errors instead of panicking.
    type Item = Expired<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        DelayQueue::poll_expired(self.get_mut(), cx)
    }
}

impl<T> wheel::Stack for Stack<T> {
    type Owned = Key;
    type Borrowed = Key;
    type Store = SlabStorage<T>;

    fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    fn push(&mut self, item: Self::Owned, store: &mut Self::Store) {
        // Ensure the entry is not already in a stack.
        debug_assert!(store[item].next.is_none());
        debug_assert!(store[item].prev.is_none());

        // Remove the old head entry
        let old = self.head.take();

        if let Some(idx) = old {
            store[idx].prev = Some(item);
        }

        store[item].next = old;
        self.head = Some(item);
    }

    fn pop(&mut self, store: &mut Self::Store) -> Option<Self::Owned> {
        if let Some(key) = self.head {
            self.head = store[key].next;

            if let Some(idx) = self.head {
                store[idx].prev = None;
            }

            store[key].next = None;
            debug_assert!(store[key].prev.is_none());

            Some(key)
        } else {
            None
        }
    }

    fn remove(&mut self, item: &Self::Borrowed, store: &mut Self::Store) {
        let key = *item;
        assert!(store.contains(item));

        // Ensure that the entry is in fact contained by the stack
        debug_assert!({
            // This walks the full linked list even if an entry is found.
            let mut next = self.head;
            let mut contains = false;

            while let Some(idx) = next {
                let data = &store[idx];

                if idx == *item {
                    debug_assert!(!contains);
                    contains = true;
                }

                next = data.next;
            }

            contains
        });

        if let Some(next) = store[key].next {
            store[next].prev = store[key].prev;
        }

        if let Some(prev) = store[key].prev {
            store[prev].next = store[key].next;
        } else {
            self.head = store[key].next;
        }

        store[key].next = None;
        store[key].prev = None;
    }

    fn when(item: &Self::Borrowed, store: &Self::Store) -> u64 {
        store[*item].when
    }
}

impl<T> Default for Stack<T> {
    fn default() -> Stack<T> {
        Stack {
            head: None,
            _p: PhantomData,
        }
    }
}

impl Key {
    pub(crate) fn new(index: usize) -> Key {
        Key { index }
    }
}

impl KeyInternal {
    pub(crate) fn new(index: usize) -> KeyInternal {
        KeyInternal { index }
    }
}

impl From<Key> for KeyInternal {
    fn from(item: Key) -> Self {
        KeyInternal::new(item.index)
    }
}

impl From<KeyInternal> for Key {
    fn from(item: KeyInternal) -> Self {
        Key::new(item.index)
    }
}

impl<T> Expired<T> {
    pub fn into_inner(self) -> T {
        self.data
    }
}
