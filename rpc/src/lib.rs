// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(
    non_exhaustive,
    integer_atomics,
    try_trait,
    nll,
    futures_api,
    pin,
    arbitrary_self_types,
    await_macro,
    async_await,
    generators,
    optin_builtin_traits,
    generator_trait,
    gen_future,
    decl_macro,
    existential_type,
)]

//! An RPC framework providing client and server.
//!
//! Features:
//! * RPC deadlines, both client- and server-side.
//! * Cascading cancellation (works with multiple hops).
//! * Tracing in the style of dapper/zipkin/opencensus (still WIP, doesn't have trace sampling yet,
//!   but the instrumentation is there).
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
//! * Pluggable transport.

#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;
#[cfg(feature = "serde")]
#[macro_use]
extern crate serde;

pub mod context;
pub mod client;
pub mod server;
pub mod transport;
pub(crate) mod util;

pub use crate::client::Client;
pub use crate::server::Server;
pub use crate::transport::Transport;

use std::{io, time::SystemTime};

/// A message from a client to a server.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ClientMessageKind<T> {
    /// A request initiated by a user. The server responds to a request by invoking a
    /// service-provided request handler.  The handler completes with a [response](Response), which
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct Request<T> {
    /// Uniquely identifies the request across all requests sent over a single channel.
    pub id: u64,
    /// The request body.
    pub message: T,
    /// When the client expects the request to be complete by. The server will cancel the request
    /// if it is not complete by this time.
    #[cfg_attr(
        feature = "serde",
        serde(serialize_with = "util::serde::serialize_epoch_secs")
    )]
    #[cfg_attr(
        feature = "serde",
        serde(deserialize_with = "util::serde::deserialize_epoch_secs")
    )]
    pub deadline: SystemTime,
}

/// A response from a server to a client.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct Response<T> {
    /// The ID of the request being responded to.
    pub request_id: u64,
    /// The response body, or an error if the request failed.
    pub message: Result<T, ServerError>,
}

/// An error response from a server to a client.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct ServerError {
    #[cfg_attr(
        feature = "serde",
        serde(serialize_with = "util::serde::serialize_io_error_kind_as_u32")
    )]
    #[cfg_attr(
        feature = "serde",
        serde(deserialize_with = "util::serde::deserialize_io_error_kind_from_u32")
    )]
    pub kind: io::ErrorKind,
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
