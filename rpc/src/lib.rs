// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

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

pub use crate::{client::Client, server::Server, transport::sealed::Transport};

use futures::task::Poll;
use std::{io, time::SystemTime};

/// A message from a client to a server.
#[derive(Debug)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub enum ClientMessage<T> {
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
        /// The trace context associates the message with a specific chain of causally-related actions,
        /// possibly orchestrated across many distributed systems.
        #[cfg_attr(feature = "serde", serde(default))]
        trace_context: trace::Context,
        /// The ID of the request to cancel.
        request_id: u64,
    },
    #[doc(hidden)]
    _NonExhaustive,
}

/// A request from a client to a server.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub struct Request<T> {
    /// Trace context, deadline, and other cross-cutting concerns.
    pub context: context::Context,
    /// Uniquely identifies the request across all requests sent over a single channel.
    pub id: u64,
    /// The request body.
    pub message: T,
    #[doc(hidden)]
    #[cfg_attr(feature = "serde1", serde(skip_serializing, default))]
    _non_exhaustive: (),
}

/// A response from a server to a client.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub struct Response<T> {
    /// The ID of the request being responded to.
    pub request_id: u64,
    /// The response body, or an error if the request failed.
    pub message: Result<T, ServerError>,
    #[doc(hidden)]
    #[cfg_attr(feature = "serde1", serde(skip_serializing, default))]
    _non_exhaustive: (),
}

/// An error response from a server to a client.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
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
    #[doc(hidden)]
    #[cfg_attr(feature = "serde1", serde(skip_serializing, default))]
    _non_exhaustive: (),
}

impl From<ServerError> for io::Error {
    fn from(e: ServerError) -> io::Error {
        io::Error::new(e.kind, e.detail.unwrap_or_default())
    }
}

impl<T> Request<T> {
    /// Returns the deadline for this request.
    pub fn deadline(&self) -> &SystemTime {
        &self.context.deadline
    }
}

pub(crate) type PollIo<T> = Poll<Option<io::Result<T>>>;
