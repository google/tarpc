// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![feature(
    weak_counts,
    non_exhaustive,
    integer_atomics,
    try_trait,
    arbitrary_self_types,
    async_await,
    trait_alias
)]
#![deny(missing_docs, missing_debug_implementations)]

//! An RPC framework providing client and server.
//!
//! Features:
//! * RPC deadlines, both client- and server-side.
//! * Cascading cancellation (works with multiple hops).
//! * Configurable limits
//!    * In-flight requests, both client and server-side.
//!        * Server-side limit is per-connection.
//!        * When the server reaches the in-flight request maximum, it returns a throttled error
//!          to the client.
//!        * When the client reaches the in-flight request max, messages are buffered up to a
//!          configurable maximum, beyond which the requests are back-pressured.
//!    * Server connections.
//!        * Total and per-IP limits.
//!        * When an incoming connection is accepted, if already at maximum, the connection is
//!          dropped.
//! * Transport agnostic.

pub mod client;
pub mod context;
pub mod server;
pub mod transport;
pub(crate) mod util;

pub use crate::{client::Client, server::Server, transport::Transport};

use futures::{
    task::{Poll, Spawn, SpawnError, SpawnExt},
    Future,
};
use std::{cell::RefCell, io, sync::Once, time::SystemTime};

/// A message from a client to a server.
#[derive(Debug)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ClientMessage<T> {
    /// The trace context associates the message with a specific chain of causally-related actions,
    /// possibly orchestrated across many distributed systems.
    pub trace_context: trace::Context,
    /// The message payload.
    pub message: ClientMessageKind<T>,
}

/// Different messages that can be sent from a client to a server.
#[derive(Debug)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum ClientMessageKind<T> {
    /// A request initiated by a user. The server responds to a request by invoking a
    /// service-provided request handler.  The handler completes with a [`response`](Response), which
    /// the server sends back to the client.
    Request(Request<T>),
    /// A command to cancel an in-flight request, automatically sent by the client when a response
    /// future is dropped.
    ///
    /// When received, the server will immediately cancel the main task (top-level future) of the
    /// request handler for the associated request. Any tasks spawned by the request handler will
    /// not be canceled, because the framework layer does not
    /// know about them.
    Cancel {
        /// The ID of the request to cancel.
        request_id: u64,
    },
}

/// A request from a client to a server.
#[derive(Debug)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Request<T> {
    /// Uniquely identifies the request across all requests sent over a single channel.
    pub id: u64,
    /// The request body.
    pub message: T,
    /// When the client expects the request to be complete by. The server will cancel the request
    /// if it is not complete by this time.
    #[cfg_attr(
        feature = "serde1",
        serde(serialize_with = "util::serde::serialize_epoch_secs")
    )]
    #[cfg_attr(
        feature = "serde1",
        serde(deserialize_with = "util::serde::deserialize_epoch_secs")
    )]
    pub deadline: SystemTime,
}

/// A response from a server to a client.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Response<T> {
    /// The ID of the request being responded to.
    pub request_id: u64,
    /// The response body, or an error if the request failed.
    pub message: Result<T, ServerError>,
}

/// An error response from a server to a client.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ServerError {
    #[cfg_attr(
        feature = "serde1",
        serde(serialize_with = "util::serde::serialize_io_error_kind_as_u32")
    )]
    #[cfg_attr(
        feature = "serde1",
        serde(deserialize_with = "util::serde::deserialize_io_error_kind_from_u32")
    )]
    /// The type of error that occurred to fail the request.
    pub kind: io::ErrorKind,
    /// A message describing more detail about the error that occurred.
    pub detail: Option<String>,
}

impl From<ServerError> for io::Error {
    fn from(e: ServerError) -> io::Error {
        io::Error::new(e.kind, e.detail.unwrap_or_default())
    }
}

impl<T> Request<T> {
    /// Returns the deadline for this request.
    pub fn deadline(&self) -> &SystemTime {
        &self.deadline
    }
}

pub(crate) type PollIo<T> = Poll<Option<io::Result<T>>>;

static INIT: Once = Once::new();
static mut SEED_SPAWN: Option<Box<dyn CloneSpawn>> = None;
thread_local! {
    static SPAWN: RefCell<Box<dyn CloneSpawn>> = {
        unsafe {
            // INIT must always be called before accessing SPAWN.
            // Otherwise, accessing SPAWN can trigger undefined behavior due to race conditions.
            INIT.call_once(|| {});
            RefCell::new(SEED_SPAWN.as_ref().expect("init() must be called.").box_clone())
        }
    };
}

/// Initializes the RPC library with a mechanism to spawn futures on the user's runtime.
/// Client stubs and servers both use the initialized spawn.
///
/// Init only has an effect the first time it is called. If called previously, successive calls to
/// init are noops.
pub fn init(spawn: impl Spawn + Clone + 'static) {
    unsafe {
        INIT.call_once(|| {
            SEED_SPAWN = Some(Box::new(spawn));
        });
    }
}

pub(crate) fn spawn(future: impl Future<Output = ()> + Send + 'static) -> Result<(), SpawnError> {
    SPAWN.with(|spawn| spawn.borrow_mut().spawn(future))
}

trait CloneSpawn: Spawn {
    fn box_clone(&self) -> Box<dyn CloneSpawn>;
}

impl<S: Spawn + Clone + 'static> CloneSpawn for S {
    fn box_clone(&self) -> Box<dyn CloneSpawn> {
        Box::new(self.clone())
    }
}
