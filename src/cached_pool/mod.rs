// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use fnv::FnvHasher;
use mio::{self, EventLoop, EventLoopConfig, Handler, Timeout};
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasherDefault;
use std::fmt;
use std::ops::AddAssign;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

lazy_static! {
    /// The thread pool global event loop on which all thread pools are registered.
    static ref REGISTRY: Registry = Registry::new();
}

/// A thread pool that grows automatically as needed, up to a maximum size.
/// Threads expire after a configurable amount of time.
#[derive(Clone, Debug)]
pub struct CachedPool {
    id: PoolId,
    registry: Registry,
    /// Whether the pool should block when dropped until all tasks complete.
    pub await_termination: bool,
}

impl CachedPool {
    /// Create a new thread pool with the given maximum number of threads
    /// and maximum idle time before thread expiration.
    pub fn new(config: Config) -> CachedPool {
        REGISTRY.register(config)
    }

    /// Submit a new task to a thread.
    ///
    /// Fails if all threads are busy with tasks, and the thread pool
    /// is running its maximum configured number of threads.
    pub fn execute<F>(&self, f: F) -> Result<(), Box<Task + Send>>
        where F: FnOnce() + Send + 'static
    {
        self.registry.execute(self.id, f)
    }

    /// Get debug information about the thread pool.
    pub fn debug(&self) -> DebugInfo {
        self.registry.debug(self.id)
    }
}

impl Drop for CachedPool {
    fn drop(&mut self) {
        if self.await_termination {
            self.registry.deregister_and_await_termination(self.id);
        } else {
            self.registry.deregister(self.id);
        }
    }
}

/// Configuration for `CachedPool`.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The maximum number of threads the thread pool can spawn at the same time.
    pub max_threads: u32,
    /// The minimum number of threads the thread pool will contain when idle.
    pub min_threads: u32,
    /// The amount of time a thread can be idle before expiring.
    pub max_idle: Duration,
}

impl Config {
    /// Creates a new `Config` with the specified max idle time for threads, and all other fields
    /// set to defaults.
    pub fn max_idle(max_idle: Duration) -> Config {
        Config { max_idle: max_idle, ..Config::default() }
    }

    /// Creates a new `Config` with the specified maximum number of threads, and all other fields
    /// set to defaults.
    pub fn max_threads(max_threads: u32) -> Config {
        Config { max_threads: max_threads, ..Config::default() }
    }

    /// Creates a new `Config` with the specified minimum number of threads, and all other fields
    /// set to defaults.
    pub fn min_threads(min_threads: u32) -> Config {
        Config { min_threads: min_threads, ..Config::default() }
    }

    fn max_idle_ms(&self) -> u64 {
        self.max_idle.as_millis().expect("Overflowed converting Duration to milliseconds!")
    }
}

impl Default for Config {
    fn default() -> Self {
        use std::u32;
        Config {
            max_threads: u32::MAX,
            min_threads: 0,
            max_idle: Duration::from_secs(5 * 60),
        }
    }
}

#[derive(Clone, Debug)]
struct Registry {
    tx: mio::Sender<EventLoopAction>,
    count: Option<Arc<()>>,
}

impl Registry {
    /// Create a new thread pool with the given maximum number of threads
    /// and maximum idle time before thread expiration.
    fn new() -> Registry {
        let mut config = EventLoopConfig::default();
        config.notify_capacity(1_000_000);
        let mut event_loop = EventLoop::configured(config).expect(pos!());
        let tx = event_loop.channel();
        thread::Builder::new()
            .name("CachedPool Registry".to_string())
            .spawn(move || {
                if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                    error!("Dispatcher: event loop failed, {:?}", e);
                }
            })
            .expect(pos!());
        Registry {
            tx: tx,
            count: Some(Arc::new(())),
        }
    }

    /// Submit a new task to a thread.
    ///
    /// Fails if all threads are busy with tasks, and the thread pool
    /// is running its maximum configured number of threads.
    fn execute<F>(&self, pool_id: PoolId, f: F) -> Result<(), Box<Task + Send>>
        where F: FnOnce() + Send + 'static
    {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Execute(pool_id, Box::new(f), tx)).expect(pos!());
        rx.recv().expect(pos!())
    }

    /// Get debug information about the thread pool.
    fn debug(&self, pool_id: PoolId) -> DebugInfo {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Debug(pool_id, tx)).expect(pos!());
        rx.recv().expect(pos!())
    }

    /// Registers a new thread pool on the event loop.
    fn register(&self, config: Config) -> CachedPool {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Register(tx, config)).expect(pos!());
        let pool_id = rx.recv().expect(pos!());
        CachedPool {
            id: pool_id,
            registry: self.clone(),
            await_termination: false,
        }
    }

    /// Deregisters a thread pool, removing it from the event loop.
    fn deregister(&self, pool_id: PoolId) {
        self.tx.send(EventLoopAction::Deregister(pool_id)).expect(pos!());
    }

    /// Deregisters a thread pool, removing it from the event loop. Blocks until all active
    /// tasks complete.
    fn deregister_and_await_termination(&self, pool_id: PoolId) {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::DeregisterAndAwaitTermination(pool_id, tx)).expect(pos!());
        rx.recv().expect(pos!());
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        match Arc::try_unwrap(self.count.take().unwrap()) {
            Ok(_) => {
                debug!("CachedPool: shutting down event loop.");
                // Safe to unwrap the send call because:
                //  1. The only thing that can notify the event loop is the CachedPool.
                //  2. All methods of notifying it besides this one do a handshake, which means
                //     the notification is popped from the buffer before the method completes.
                //  3. Thus, we know that when dropping the CachedPool, it's impossible to get
                //     a filled buffer error.
                self.tx.send(EventLoopAction::Shutdown).expect(pos!());
            }
            Err(count) => self.count = Some(count),
        }
    }
}

/// A thin wrapper around u64 to disambiguate pool id from thread id.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PoolId(u64);

impl AddAssign<u64> for PoolId {
    fn add_assign(&mut self, amount: u64) {
        self.0 += amount;
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct ThreadId(u64);

impl AddAssign<u64> for ThreadId {
    fn add_assign(&mut self, amount: u64) {
        self.0 += amount;
    }
}


struct Pool {
    id: PoolId,
    threads: HashMap<ThreadId, ThreadHandle, BuildHasherDefault<FnvHasher>>,
    queue: VecDeque<ThreadId>,
    max_idle_ms: u64,
    min_threads: u32,
    max_threads: u32,
    next_thread_id: ThreadId,
}

impl Pool {
    fn spawn(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> Option<ThreadId> {
        let (tx, rx) = mpsc::channel();
        if self.threads.len() == self.max_threads as usize {
            return None;
        }
        let thread_id = self.next_thread_id;
        self.next_thread_id += 1;
        let thread_handle = ThreadHandle {
            tx: tx,
            timeout: None,
        };
        self.threads.insert(thread_id, thread_handle);
        let event_loop_tx = event_loop.channel();
        let pool_id = self.id;
        let thread = Thread {
            event_loop_tx: event_loop_tx,
            rx: rx,
            pool_id: pool_id,
            id: thread_id,
        };
        thread::Builder::new()
            .name(format!("{:?}/{:?}", pool_id, thread_id))
            .spawn(move || thread.run())
            .expect(pos!());
        Some(thread_id)
    }

    fn execute(&mut self,
               task: Box<Task + Send>,
               tx: mpsc::Sender<Result<(), Box<Task + Send>>>,
               event_loop: &mut EventLoop<Dispatcher>) {
        loop {
            match self.queue.pop_front() {
                // No idle threads.
                None => {
                    match self.spawn(event_loop) {
                        // Max threads spawned.
                        None => {
                            tx.send(Err(task)).expect(pos!());
                        }
                        Some(thread_id) => {
                            self.threads
                                .get_mut(&thread_id)
                                .expect(pos!())
                                .execute(task, event_loop)
                                .expect(pos!());
                            tx.send(Ok(())).expect(pos!());
                        }
                    }
                    break;
                }
                Some(thread_id) => {
                    if let Some(thread) = self.threads.get_mut(&thread_id) {
                        thread.execute(task, event_loop).unwrap();
                        tx.send(Ok(())).expect(pos!());
                        break;
                    } else {
                        debug!("Skipping expired thread {:?}.", thread_id);
                    }
                }
            }
        }
    }

    fn enqueue(&mut self, token: ThreadId, event_loop: &mut EventLoop<Dispatcher>) {
        if self.threads.len() > self.min_threads as usize {
            let timeout = event_loop.timeout_ms((self.id, token), self.max_idle_ms).expect(pos!());
            self.threads.get_mut(&token).expect(pos!()).timeout = Some(timeout);
        }
        self.queue.push_back(token);
    }
}

struct Thread {
    event_loop_tx: mio::Sender<EventLoopAction>,
    rx: mpsc::Receiver<ThreadAction>,
    pool_id: PoolId,
    id: ThreadId,
}

impl Drop for Thread {
    fn drop(&mut self) {
        if thread::panicking() {
            info!("Thread {:?}: panicked.", self.id());
            let _ = self.event_loop_tx.send(EventLoopAction::RemovePanicked(self.pool_id, self.id));
        }
    }
}

impl Thread {
    fn run(self) {
        loop {
            match self.rx.recv() {
                Err(_) |
                Ok(ThreadAction::Expire) => {
                    debug!("Thread {:?}: expired.", self.id());
                    break;
                }
                Ok(ThreadAction::ExpireHandshake(tx)) => {
                    debug!("Thread {:?}: expired.", self.id());
                    tx.send(()).expect(pos!());
                    break;
                }
                Ok(ThreadAction::Execute(task)) => {
                    debug!("Thread {:?}: received work.", self.id());
                    task.run();
                    if let Err(_) = self.event_loop_tx
                        .send(EventLoopAction::Enqueue(self.pool_id, self.id)) {
                        break;
                    }
                }
            }
        }
    }

    fn id(&self) -> (PoolId, ThreadId) {
        (self.pool_id, self.id)
    }
}

struct ThreadHandle {
    tx: mpsc::Sender<ThreadAction>,
    timeout: Option<Timeout>,
}

impl ThreadHandle {
    fn execute(&mut self,
               task: Box<Task + Send>,
               event_loop: &mut EventLoop<Dispatcher>)
               -> Result<(), mpsc::SendError<ThreadAction>> {
        try!(self.send(ThreadAction::Execute(task)));
        self.clear_timeout(event_loop);
        Ok(())
    }

    fn expire(&mut self, event_loop: &mut EventLoop<Dispatcher>) {
        self.clear_timeout(event_loop);
        // Not guaranteed that the thread will complete -- the task could panic
        // at any point. If it panicked that's fine, though, since dead is dead.
        let _ = self.send(ThreadAction::Expire);
    }

    fn expire_and_await(&mut self, event_loop: &mut EventLoop<Dispatcher>) {
        self.clear_timeout(event_loop);
        let (tx, rx) = mpsc::channel();
        if let Ok(_) = self.send(ThreadAction::ExpireHandshake(tx)) {
            // Not guaranteed that the thread will complete -- the task could panic
            // at any point. So just do our best to check that it completed.
            let _ = rx.recv();
        }
    }

    fn send(&self, action: ThreadAction) -> Result<(), mpsc::SendError<ThreadAction>> {
        self.tx.send(action)
    }

    fn clear_timeout(&mut self, event_loop: &mut EventLoop<Dispatcher>) {
        if let Some(timeout) = self.timeout.take() {
            event_loop.clear_timeout(timeout);
        }
    }
}

/// A runnable task.
pub trait Task {
    /// Run the task.
    fn run(self: Box<Self>);
}

impl fmt::Debug for Box<Task + Send> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Box<Task + Send>")
    }
}

impl<F> Task for F
    where F: FnOnce() + Send
{
    fn run(self: Box<F>) {
        self()
    }
}

enum ThreadAction {
    Expire,
    ExpireHandshake(mpsc::Sender<()>),
    Execute(Box<Task + Send>),
}

enum EventLoopAction {
    Enqueue(PoolId, ThreadId),
    RemovePanicked(PoolId, ThreadId),
    Execute(PoolId, Box<Task + Send>, mpsc::Sender<Result<(), Box<Task + Send>>>),
    Debug(PoolId, mpsc::Sender<DebugInfo>),
    Deregister(PoolId),
    DeregisterAndAwaitTermination(PoolId, mpsc::Sender<()>),
    Register(mpsc::Sender<PoolId>, Config),
    Shutdown,
}

/// Information about the thread pool.
#[derive(Clone, Debug)]
pub struct DebugInfo {
    /// The id of the thread pool.
    pub id: PoolId,
    /// The total number of alive threads.
    pub count: usize,
}

struct Dispatcher {
    pools: HashMap<PoolId, Pool, BuildHasherDefault<FnvHasher>>,
    next_pool_id: PoolId,
}

impl Dispatcher {
    fn new() -> Dispatcher {
        Dispatcher {
            pools: HashMap::with_hasher(BuildHasherDefault::default()),
            next_pool_id: PoolId(0),
        }
    }
}

impl Handler for Dispatcher {
    type Timeout = (PoolId, ThreadId);
    type Message = EventLoopAction;

    fn notify(&mut self, event_loop: &mut EventLoop<Dispatcher>, action: EventLoopAction) {
        match action {
            // Impossible for a registry to send a shutdown message before all pools have
            // been deregistered, so should be fine to simply shutdown.
            EventLoopAction::Shutdown => event_loop.shutdown(),
            EventLoopAction::Register(tx, config) => {
                let pool_id = self.next_pool_id;
                self.next_pool_id += 1;
                self.pools.insert(pool_id,
                                  Pool {
                                      id: pool_id,
                                      threads: HashMap::with_hasher(BuildHasherDefault::default()),
                                      queue: VecDeque::new(),
                                      max_threads: config.max_threads,
                                      min_threads: config.min_threads,
                                      max_idle_ms: config.max_idle_ms(),
                                      next_thread_id: ThreadId(0),
                                  });
                tx.send(pool_id).unwrap();
            }
            EventLoopAction::Deregister(pool_id) => {
                for thread in self.pools.remove(&pool_id).unwrap().threads.values_mut() {
                    thread.expire(event_loop);
                }
            }
            EventLoopAction::DeregisterAndAwaitTermination(pool_id, tx) => {
                for thread in self.pools.remove(&pool_id).unwrap().threads.values_mut() {
                    thread.expire_and_await(event_loop);
                }
                tx.send(()).expect(pos!());
            }
            EventLoopAction::Debug(pool_id, tx) => {
                tx.send(DebugInfo {
                        id: pool_id,
                        count: self.pools[&pool_id].threads.len(),
                    })
                    .expect(pos!());
            }
            EventLoopAction::Enqueue(pool_id, token) => {
                // It's possible that a thread was working when the pool was dropped. In that case
                // it will have been sent an expire message, so we don't have to worry about doing
                // anything now.
                if let Some(pool) = self.pools.get_mut(&pool_id) {
                    pool.enqueue(token, event_loop);
                }
            }
            EventLoopAction::RemovePanicked(pool_id, thread_id) => {
                if let Some(pool) = self.pools.get_mut(&pool_id) {
                    pool.threads.remove(&thread_id).expect(pos!());
                }
            }
            EventLoopAction::Execute(pool_id, task, tx) => {
                let pool = self.pools.get_mut(&pool_id).unwrap();
                pool.execute(task, tx, event_loop);
            }
        }
    }

    fn timeout(&mut self,
               event_loop: &mut EventLoop<Dispatcher>,
               (pool_id, thread): Self::Timeout) {
        self.pools
            .get_mut(&pool_id)
            .expect(pos!())
            .threads
            .remove(&thread)
            .expect(pos!())
            .expire(event_loop);
    }
}

trait AsMillis {
    fn as_millis(&self) -> Option<u64>;
}

impl AsMillis for Duration {
    fn as_millis(&self) -> Option<u64> {
        const NANOS_PER_MILLI: u32 = 1_000_000;
        const MILLIS_PER_SEC: u64 = 1_000;

        let secs = if let Some(secs) = self.as_secs().checked_mul(MILLIS_PER_SEC) {
            secs
        } else {
            return None;
        };
        Some(secs + ((self.subsec_nanos() / NANOS_PER_MILLI) as u64))
    }
}

// Tests whether it's safe to drop a pool before thread execution completes.
#[test]
fn drop_safe() {
    let config = Config {
        max_threads: 1,
        max_idle: Duration::from_millis(100),
        ..Config::default()
    };
    let pool = CachedPool::new(config);
    pool.execute(|| thread::sleep(Duration::from_millis(5))).unwrap();
    drop(pool);
    thread::sleep(Duration::from_millis(100));
    // If the dispatcher panicked, created a new pool will panic, as well.
    CachedPool::new(config);
}

#[test]
fn panic_safe() {
    let pool = CachedPool::new(Config {
        max_threads: 1,
        max_idle: Duration::from_millis(100),
        ..Config::default()
    });
    pool.execute(|| panic!()).unwrap();
    thread::sleep(Duration::from_millis(100));
    pool.execute(|| {}).unwrap();
}

#[test]
fn await_termination() {
    use std::time::Instant;
    let mut pool = CachedPool::new(Config {
        max_threads: 1,
        max_idle: Duration::from_millis(100),
        ..Config::default()
    });
    pool.await_termination = true;
    pool.execute(|| thread::sleep(Duration::from_millis(100))).unwrap();
    let start = Instant::now();
    drop(pool);
    assert!(start.elapsed() > Duration::from_millis(50));
}
