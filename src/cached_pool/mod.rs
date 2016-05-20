use mio::{self, EventLoop, Handler, Timeout};
use slab;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::thread;

/// A thread pool that grows automatically as needed, up to a maximum size.
/// Threads expire after a configurable amount of time.
#[derive(Clone, Debug)]
pub struct ThreadPool {
    tx: mio::Sender<EventLoopAction>,
}

impl ThreadPool {
    /// Create a new thread pool with the given maximum number of threads
    /// and maximum idle time before thread expiration.
    pub fn new(max_threads: usize, max_idle_ms: u64) -> ThreadPool {
        let mut event_loop = EventLoop::new().expect(pos!());
        let tx = event_loop.channel();
        thread::spawn(move || {
            let _ = event_loop.run(&mut Pool {
                threads: Slab::new(max_threads),
                queue: VecDeque::new(),
                max_idle_ms: max_idle_ms,
            });
        });
        ThreadPool { tx: tx }
    }

    /// Submit a new task to a thread.
    ///
    /// Fails if all threads are busy with tasks, and the thread pool
    /// is running its maximum configured number of threads.
    pub fn execute<F>(&self, f: F) -> Result<(), Box<Task + Send>>
        where F: FnOnce() + Send + 'static
    {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Execute(Box::new(f), tx)).expect(pos!());
        rx.recv().expect(pos!())
    }

    /// Get debug informatin about the thread pool.
    pub fn debug(&self) -> DebugInfo {
        let (tx, rx) = mpsc::channel();
        self.tx.send(EventLoopAction::Debug(tx)).expect(pos!());
        rx.recv().expect(pos!())
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
    threads: Slab<ThreadHandle>,
    queue: VecDeque<Token>,
    max_idle_ms: u64,
}

impl Pool {
    /// Returns true if the thread was spawned.
    fn spawn(&mut self, event_loop: &mut EventLoop<Self>) -> Option<Token> {
        let (tx, rx) = mpsc::channel();
        let vacancy = match self.threads.vacant_entry() {
            None => return None,
            Some(vacancy) => vacancy,
        };
        let token = vacancy.index();
        // TODO(tikue): don't unwrap timeout_ms!
        let timeout = event_loop.timeout_ms(token, self.max_idle_ms).expect(pos!());
        let thread_handle = ThreadHandle {
            tx: tx,
            timeout: RefCell::new(Some(timeout)),
        };
        vacancy.insert(thread_handle);
        let event_loop_tx = event_loop.channel();
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
                        if let Err(_) = event_loop_tx.send(EventLoopAction::Enqueue(token)) {
                            break;
                        }
                    }
                }
            }
        });
        Some(token)
    }

    fn enqueue(&mut self, token: Token, event_loop: &mut EventLoop<Self>) {
        let timeout = event_loop.timeout_ms(token, self.max_idle_ms).expect(pos!());
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
               event_loop: &mut EventLoop<Pool>)
               -> Result<(), mpsc::SendError<ThreadAction>> {
        try!(self.tx.send(ThreadAction::Execute(task)));
        event_loop.clear_timeout(self.timeout.borrow_mut().take().expect(pos!()));
        Ok(())
    }

    fn expire(&self) {
        self.tx.send(ThreadAction::Expire).expect(pos!())
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
    Enqueue(Token),
    Execute(Box<Task + Send>, mpsc::Sender<Result<(), Box<Task + Send>>>),
    Debug(mpsc::Sender<DebugInfo>),
}

/// Information about the thread pool.
#[derive(Clone, Debug)]
pub struct DebugInfo {
    /// The total number of alive threads.
    pub count: usize,
}

impl Handler for Pool {
    type Timeout = Token;
    type Message = EventLoopAction;

    fn notify(&mut self, event_loop: &mut EventLoop<Self>, action: EventLoopAction) {
        match action {
            EventLoopAction::Debug(tx) => {
                let _ = tx.send(DebugInfo { count: self.threads.count() });
            }
            EventLoopAction::Enqueue(token) => self.enqueue(token, event_loop),
            EventLoopAction::Execute(task, tx) => {
                loop {
                    match self.queue.pop_front() {
                        // No idle threads.
                        None => {
                            match self.spawn(event_loop) {
                                // Max threads spawned.
                                None => {
                                    let _ = tx.send(Err(task));
                                }
                                Some(token) => {
                                    self.threads[token].execute(task, event_loop).expect(pos!());
                                    let _ = tx.send(Ok(()));
                                }
                            }
                            break;
                        }
                        Some(token) => {
                            if let Some(thread) = self.threads.get(token) {
                                thread.execute(task, event_loop).unwrap();
                                let _ = tx.send(Ok(()));
                                break;
                            } else {
                                debug!("Skipping expired thread {:?}.", token);
                            }
                        }
                    }
                }
            }
        }
    }

    fn timeout(&mut self, _: &mut EventLoop<Self>, thread: Token) {
        self.threads.remove(thread).expect(pos!()).expire();
    }
}

#[test]
fn it_works() {
    extern crate env_logger;
    use std::time::Duration;
    let _ = env_logger::init();
    let pool = ThreadPool::new(1000, 1000);
    for _ in 0..15 {
        pool.execute(move || {
                thread::sleep(Duration::from_secs(5));
            })
            .ok()
            .unwrap();
        info!("{:?}", pool.debug());
        thread::sleep(Duration::from_millis(500));
    }
    for _ in 0..7 {
        pool.execute(move || {
                thread::sleep(Duration::from_secs(5));
            })
            .ok()
            .unwrap();
        info!("{:?}", pool.debug());
        thread::sleep(Duration::from_secs(1));
    }
    info!("Almost done...");
    thread::sleep(Duration::from_millis(5500));
    info!("Done.");
}
