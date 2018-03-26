use futures::unsync;
use std::io;
use tokio_service::{NewService, Service};

#[derive(Debug)]
pub enum Action {
    Increment,
    Decrement,
}

#[derive(Clone, Debug)]
pub struct Tracker {
    pub tx: unsync::mpsc::UnboundedSender<Action>,
}

impl Tracker {
    pub fn pair() -> (Self, unsync::mpsc::UnboundedReceiver<Action>) {
        let (tx, rx) = unsync::mpsc::unbounded();
        (Self { tx }, rx)
    }

    pub fn increment(&self) {
        let _ = self.tx.unbounded_send(Action::Increment);
    }

    pub fn decrement(&self) {
        debug!("Closing connection");
        let _ = self.tx.unbounded_send(Action::Decrement);
    }
}

#[derive(Debug)]
pub struct TrackingService<S> {
    pub service: S,
    pub tracker: Tracker,
}

#[derive(Debug)]
pub struct TrackingNewService<S> {
    pub new_service: S,
    pub connection_tracker: Tracker,
}

impl<S: Service> Service for TrackingService<S> {
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, req: Self::Request) -> Self::Future {
        trace!("Calling service.");
        self.service.call(req)
    }
}

impl<S> Drop for TrackingService<S> {
    fn drop(&mut self) {
        debug!("Dropping ConnnectionTrackingService.");
        self.tracker.decrement();
    }
}

impl<S: NewService> NewService for TrackingNewService<S> {
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Instance = TrackingService<S::Instance>;

    fn new_service(&self) -> io::Result<Self::Instance> {
        self.connection_tracker.increment();
        Ok(TrackingService {
            service: self.new_service.new_service()?,
            tracker: self.connection_tracker.clone(),
        })
    }
}
