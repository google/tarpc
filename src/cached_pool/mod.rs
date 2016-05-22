// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::{self, EventLoop, EventLoopConfig, Handler, Timeout};
use slab;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, mpsc};
use std::thread;

lazy_static! {
    /// The thread pool global event loop on which all thread pools are registered.
    static ref REGISTRY: Registry = Registry::new();
}

/// A thread pool that grows automatically as needed, up to a maximum size.
/// Threads expire after a configurable amount of time.
#[derive(Clone, Debug)]
pub struct CachedPool {
    id: u64,
    registry: Registry,
}

impl CachedPool {
    /// Create a new thread pool with the given maximum number of threads
    /// and maximum idle time before thread expiration.
    pub fn new(max_threads: usize, max_idle_ms: u64) -> CachedPool {
        REGISTRY.register(max_threads, max_idle_ms)
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
        self.registry.deregister(self.id);
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
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Dispatcher: event loop failed, {:?}", e);
            }
        });
        Registry {
            tx: tx,
            count: Some(Arc::new(())),
        }
    }

    /// Submit a new task to a thread.
    ///
    /// Fails if all threads are busy with tasks, and the thread pool
    /// is running its maximum configured number of threads.
    fn execute<F>(&self, pool_id: u64, f: F) -> Result<(), Box<Task + Send>>
        where F: FnOnce() + Send + 'static
    {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Execute(pool_id, Box::new(f), tx)).expect(pos!());
        rx.recv().expect(pos!())
    }

    /// Get debug information about the thread pool.
    fn debug(&self, pool_id: u64) -> DebugInfo {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Debug(pool_id, tx)).expect(pos!());
        rx.recv().expect(pos!())
    }

    /// Registers a new thread pool on the event loop.
    fn register(&self, max_threads: usize, max_idle_ms: u64) -> CachedPool {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(EventLoopAction::Register {
                tx: tx,
                max_threads: max_threads,
                max_idle_ms: max_idle_ms,
            })
            .expect(pos!());
        let pool_id = rx.recv().expect(pos!());
        CachedPool {
            id: pool_id,
            registry: self.clone(),
        }
    }

    /// Deregisters a thread pool, removing it from the event loop.
    fn deregister(&self, pool_id: u64) {
        self.tx.send(EventLoopAction::Deregister(pool_id)).expect(pos!());
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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct Token(u64);

impl slab::Index for Token {
    fn from_usize(i: usize) -> Self {
        Token(i as u64)
    }

    fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

type Slab<T> = slab::Slab<T, Token>;

struct Pool {
    id: u64,
    threads: Slab<ThreadHandle>,
    queue: VecDeque<Token>,
    max_idle_ms: u64,
}

impl Pool {
    fn spawn(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> Option<Token> {
        let (tx, rx) = mpsc::channel();
        let vacancy = match self.threads.vacant_entry() {
            None => return None,
            Some(vacancy) => vacancy,
        };
        let token = vacancy.index();
        // TODO(tikue): don't unwrap timeout_ms!
        let timeout = event_loop.timeout_ms((self.id, token), self.max_idle_ms).expect(pos!());
        let thread_handle = ThreadHandle {
            tx: tx,
            timeout: RefCell::new(Some(timeout)),
        };
        vacancy.insert(thread_handle);
        let event_loop_tx = event_loop.channel();
        let pool_id = self.id;
        thread::spawn(move || {
            loop {
                match rx.recv() {
                    Err(_) |
                    Ok(ThreadAction::Expire) => {
                        debug!("Thread {:?} expired.", token);
                        break;
                    }
                    Ok(ThreadAction::Execute(task)) => {
                        debug!("Thread {:?} received work.", token);
                        task.run();
                        if let Err(_) = event_loop_tx.send(EventLoopAction::Enqueue(pool_id,
                                                                                    token)) {
                            break;
                        }
                    }
                }
            }
        });
        Some(token)
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
                        Some(token) => {
                            self.threads[token].execute(task, event_loop).expect(pos!());
                            tx.send(Ok(())).expect(pos!());
                        }
                    }
                    break;
                }
                Some(token) => {
                    if let Some(thread) = self.threads.get(token) {
                        thread.execute(task, event_loop).unwrap();
                        tx.send(Ok(())).expect(pos!());
                        break;
                    } else {
                        debug!("Skipping expired thread {:?}.", token);
                    }
                }
            }
        }
    }

    fn enqueue(&mut self, token: Token, event_loop: &mut EventLoop<Dispatcher>) {
        let timeout = event_loop.timeout_ms((self.id, token), self.max_idle_ms).expect(pos!());
        *self.threads[token].timeout.borrow_mut() = Some(timeout);
        self.queue.push_back(token);
    }
}

struct ThreadHandle {
    tx: mpsc::Sender<ThreadAction>,
    timeout: RefCell<Option<Timeout>>,
}

impl ThreadHandle {
    fn execute(&self,
               task: Box<Task + Send>,
               event_loop: &mut EventLoop<Dispatcher>)
               -> Result<(), mpsc::SendError<ThreadAction>> {
        try!(self.send(ThreadAction::Execute(task)));
        event_loop.clear_timeout(self.timeout.borrow_mut().take().expect(pos!()));
        Ok(())
    }

    fn expire(&self) -> Result<(), mpsc::SendError<ThreadAction>> {
        self.send(ThreadAction::Expire)
    }

    fn send(&self, action: ThreadAction) -> Result<(), mpsc::SendError<ThreadAction>> {
        self.tx.send(action)
    }
}

/// A runnable task.
pub trait Task {
    /// Run the task.
    fn run(self: Box<Self>);
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
    Execute(Box<Task + Send>),
}

enum EventLoopAction {
    Enqueue(u64, Token),
    Execute(u64, Box<Task + Send>, mpsc::Sender<Result<(), Box<Task + Send>>>),
    Debug(u64, mpsc::Sender<DebugInfo>),
    Deregister(u64),
    Register {
        tx: mpsc::Sender<u64>,
        max_threads: usize,
        max_idle_ms: u64,
    },
    Shutdown,
}

/// Information about the thread pool.
#[derive(Clone, Debug)]
pub struct DebugInfo {
    /// The id of the thread pool.
    pub id: u64,
    /// The total number of alive threads.
    pub count: usize,
}

struct Dispatcher {
    pools: HashMap<u64, Pool>,
    next_pool_id: u64,
}

impl Dispatcher {
    fn new() -> Dispatcher {
        Dispatcher {
            pools: HashMap::new(),
            next_pool_id: 0,
        }
    }
}

impl Handler for Dispatcher {
    type Timeout = (u64, Token);
    type Message = EventLoopAction;

    fn notify(&mut self, event_loop: &mut EventLoop<Dispatcher>, action: EventLoopAction) {
        match action {
            EventLoopAction::Shutdown => event_loop.shutdown(),
            EventLoopAction::Register { tx, max_threads, max_idle_ms } => {
                let pool_id = self.next_pool_id;
                self.next_pool_id += 1;
                self.pools.insert(pool_id,
                                  Pool {
                                      id: pool_id,
                                      threads: Slab::new(max_threads),
                                      queue: VecDeque::new(),
                                      max_idle_ms: max_idle_ms,
                                  });
                tx.send(pool_id).unwrap();
            }
            EventLoopAction::Deregister(pool_id) => {
                for thread in self.pools.remove(&pool_id).unwrap().threads.iter() {
                    thread.expire().expect(pos!());
                }
            }
            EventLoopAction::Debug(pool_id, tx) => {
                tx.send(DebugInfo {
                      id: pool_id,
                      count: self.pools[&pool_id].threads.count(),
                  })
                  .expect(pos!());
            }
            EventLoopAction::Enqueue(pool_id, token) => {
                self.pools.get_mut(&pool_id).expect(pos!()).enqueue(token, event_loop);
            }
            EventLoopAction::Execute(pool_id, task, tx) => {
                let pool = self.pools.get_mut(&pool_id).unwrap();
                pool.execute(task, tx, event_loop);
            }
        }
    }

    fn timeout(&mut self, _: &mut EventLoop<Dispatcher>, (pool_id, thread): Self::Timeout) {
        self.pools
            .get_mut(&pool_id)
            .expect(pos!())
            .threads
            .remove(thread)
            .expect(pos!())
            .expire()
            .expect(pos!());
    }
}

#[test]
#[ignore]
fn it_works() {
    extern crate env_logger;
    use std::time::Duration;
    let _ = env_logger::init();

    let pools = &[CachedPool::new(1000, 5000), CachedPool::new(1000, 500)];
    for _ in 0..15 {
        for pool in pools {
            pool.execute(move || {
                    thread::sleep(Duration::from_secs(5));
                })
                .ok()
                .unwrap();
        }
        info!("{:?}",
              pools.iter().map(CachedPool::debug).collect::<Vec<_>>());
        thread::sleep(Duration::from_millis(500));
    }
    for _ in 0..7 {
        for pool in pools {
            pool.execute(move || {
                    thread::sleep(Duration::from_secs(5));
                })
                .ok()
                .unwrap();
        }
        info!("{:?}",
              pools.iter().map(CachedPool::debug).collect::<Vec<_>>());
        thread::sleep(Duration::from_secs(1));
    }
    info!("Almost done...");
    thread::sleep(Duration::from_millis(5500));
    info!("Done.");
}
