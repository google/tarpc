use crate::{
    context,
    util::{Compact, TimeUntil},
    PollIo, Response, ServerError,
};
use fnv::FnvHashMap;
use futures::ready;
use log::{debug, trace};
use std::{
    collections::hash_map,
    io,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tokio_util::time::delay_queue::{self, DelayQueue};

/// Requests already written to the wire that haven't yet received responses.
#[derive(Debug)]
pub struct InFlightRequests<Resp> {
    request_data: FnvHashMap<u64, RequestData<Resp>>,
    deadlines: DelayQueue<u64>,
}

impl<Resp> Default for InFlightRequests<Resp> {
    fn default() -> Self {
        Self {
            request_data: Default::default(),
            deadlines: Default::default(),
        }
    }
}

#[derive(Debug)]
struct RequestData<Resp> {
    ctx: context::Context,
    response_completion: oneshot::Sender<Response<Resp>>,
    /// The key to remove the timer for the request's deadline.
    deadline_key: delay_queue::Key,
}

/// An error returned when an attempt is made to insert a request with an ID that is already in
/// use.
#[derive(Debug)]
pub struct AlreadyExistsError;

impl<Resp> InFlightRequests<Resp> {
    /// Returns the number of in-flight requests.
    pub fn len(&self) -> usize {
        self.request_data.len()
    }

    /// Returns true iff there are no requests in flight.
    pub fn is_empty(&self) -> bool {
        self.request_data.is_empty()
    }

    /// Starts a request, unless a request with the same ID is already in flight.
    pub fn insert_request(
        &mut self,
        request_id: u64,
        ctx: context::Context,
        response_completion: oneshot::Sender<Response<Resp>>,
    ) -> Result<(), AlreadyExistsError> {
        match self.request_data.entry(request_id) {
            hash_map::Entry::Vacant(vacant) => {
                let timeout = ctx.deadline.time_until();
                trace!(
                    "[{}] Queuing request with timeout {:?}.",
                    ctx.trace_id(),
                    timeout,
                );

                let deadline_key = self.deadlines.insert(request_id, timeout);
                vacant.insert(RequestData {
                    ctx,
                    response_completion,
                    deadline_key,
                });
                Ok(())
            }
            hash_map::Entry::Occupied(_) => Err(AlreadyExistsError),
        }
    }

    /// Removes a request without aborting. Returns true iff the request was found.
    pub fn complete_request(&mut self, response: Response<Resp>) -> bool {
        if let Some(request_data) = self.request_data.remove(&response.request_id) {
            self.request_data.compact(0.1);

            trace!("[{}] Received response.", request_data.ctx.trace_id());
            self.deadlines.remove(&request_data.deadline_key);
            request_data.complete(response);
            return true;
        }

        debug!(
            "No in-flight request found for request_id = {}.",
            response.request_id
        );

        // If the response completion was absent, then the request was already canceled.
        false
    }

    /// Cancels a request without completing (typically used when a request handle was dropped
    /// before the request completed).
    pub fn cancel_request(&mut self, request_id: u64) -> Option<context::Context> {
        if let Some(request_data) = self.request_data.remove(&request_id) {
            self.request_data.compact(0.1);
            trace!("[{}] Cancelling request.", request_data.ctx.trace_id());
            self.deadlines.remove(&request_data.deadline_key);
            Some(request_data.ctx)
        } else {
            None
        }
    }

    /// Yields a request that has expired, completing it with a TimedOut error.
    /// The caller should send cancellation messages for any yielded request ID.
    pub fn poll_expired(&mut self, cx: &mut Context) -> PollIo<u64> {
        Poll::Ready(match ready!(self.deadlines.poll_expired(cx)) {
            Some(Ok(expired)) => {
                let request_id = expired.into_inner();
                if let Some(request_data) = self.request_data.remove(&request_id) {
                    self.request_data.compact(0.1);
                    request_data.complete(Self::deadline_exceeded_error(request_id));
                }
                Some(Ok(request_id))
            }
            Some(Err(e)) => Some(Err(io::Error::new(io::ErrorKind::Other, e))),
            None => None,
        })
    }

    fn deadline_exceeded_error(request_id: u64) -> Response<Resp> {
        Response {
            request_id,
            message: Err(ServerError {
                kind: io::ErrorKind::TimedOut,
                detail: Some("Client dropped expired request.".to_string()),
            }),
        }
    }
}

/// When InFlightRequests is dropped, any outstanding requests are completed with a
/// deadline-exceeded error.
impl<Resp> Drop for InFlightRequests<Resp> {
    fn drop(&mut self) {
        let deadlines = &mut self.deadlines;
        for (_, request_data) in self.request_data.drain() {
            let expired = deadlines.remove(&request_data.deadline_key);
            request_data.complete(Self::deadline_exceeded_error(expired.into_inner()));
        }
    }
}

impl<Resp> RequestData<Resp> {
    fn complete(self, response: Response<Resp>) {
        let _ = self.response_completion.send(response);
    }
}
