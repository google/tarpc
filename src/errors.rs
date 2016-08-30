// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {bincode, futures};
use std::{fmt, io};
use std::error::Error as StdError;
use tokio_proto::proto::pipeline;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// All errors that can occur during the use of tarpc.
#[derive(Debug)]
pub enum Error<E>
    where E: SerializableError
{
    /// No address found for the specified address.
    ///
    /// Depending on the outcome of address resolution, `ToSocketAddrs` may not yield any
    /// values, which will propagate as this variant.
    NoAddressFound,
    /// Any IO error.
    Io(io::Error),
    /// Error in deserializing a server response.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize` or
    /// `serde::Deserialize`.
    ClientDeserialize(bincode::serde::DeserializeError),
    /// Error in serializing a client request.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize` or
    /// `serde::Deserialize`.
    ClientSerialize(bincode::serde::SerializeError),
    /// Error in deserializing a client request.
    ServerDeserialize(String),
    /// Error in serializing a server response.
    ServerSerialize(String),
    /// The server canceled the response before it was completed.
    ReplyCanceled,
    /// The server was unable to reply to the rpc for some reason.
    App(E),
}

impl<E: SerializableError> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::NoAddressFound | Error::ReplyCanceled => write!(f, "{}", self.description()),
            Error::ClientDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::ClientSerialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::ServerDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::ServerSerialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::App(ref e) => fmt::Display::fmt(e, f),
            Error::Io(ref e) => fmt::Display::fmt(e, f),
        }
    }
}

impl<E: SerializableError> StdError for Error<E> {
    fn description(&self) -> &str {
        match *self {
            Error::NoAddressFound => "No addresses were returned by `ToSocketAddrs::to_socket_addrs`.",
            Error::ClientDeserialize(_) => "The client failed to deserialize the server response.",
            Error::ClientSerialize(_) => "The client failed to serialize the request.",
            Error::ServerDeserialize(_) => "The server failed to deserialize the request.",
            Error::ServerSerialize(_) => "The server failed to serialize the response.",
            Error::ReplyCanceled => "The server canceled sending a response.",
            Error::App(ref e) => e.description(),
            Error::Io(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Error::ClientDeserialize(ref e) => e.cause(),
            Error::ClientSerialize(ref e) => e.cause(),
            Error::NoAddressFound |
            Error::ServerDeserialize(_) |
            Error::ServerSerialize(_) |
            Error::ReplyCanceled |
            Error::App(_) => None,
            Error::Io(ref e) => e.cause(),
        }
    }
}

impl<E: SerializableError> From<pipeline::Error<Error<E>>> for Error<E> {
    fn from(err: pipeline::Error<Error<E>>) -> Self {
        match err {
            pipeline::Error::Transport(e) => e,
            pipeline::Error::Io(e) => e.into(),
        }
    }
}

impl<E: SerializableError> From<io::Error> for Error<E> {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl<E: SerializableError> From<WireError<E>> for Error<E> {
    fn from(err: WireError<E>) -> Self {
        match err {
            WireError::ReplyCanceled => Error::ReplyCanceled,
            WireError::ServerDeserialize(s) => Error::ServerDeserialize(s),
            WireError::ServerSerialize(s) => Error::ServerSerialize(s),
            WireError::App(e) => Error::App(e),
        }
    }
}

/// A serializable, server-supplied error.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum WireError<E>
    where E: SerializableError
{
    /// The server canceled the response before it was completed.
    ReplyCanceled,
    /// Error in deserializing a client request.
    ServerDeserialize(String),
    /// Error in serializing server response.
    ServerSerialize(String),
    /// The server was unable to reply to the rpc for some reason.
    App(E),
}

impl<E> From<futures::Canceled> for WireError<E>
    where E: SerializableError
{
    fn from(_: futures::Canceled) -> Self {
        WireError::ReplyCanceled
    }
}

/// A serializable error.
pub trait SerializableError: StdError + Deserialize + Serialize + Send + 'static {}
impl<E: StdError + Deserialize + Serialize + Send + 'static> SerializableError for E {}

/// A bottom type that impls Error.
#[derive(Debug)]
pub struct Never(!);

impl StdError for Never {
    fn description(&self) -> &str {
        unreachable!()
    }

    fn cause(&self) -> Option<&StdError> {
        unreachable!()
    }
}

impl fmt::Display for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unreachable!()
    }
}

impl Serialize for Never {
    fn serialize<S>(&self, _: &mut S) -> Result<(), S::Error>
        where S: Serializer
    {
        unreachable!()
    }
}

// Please don't try to deserialize this. :(
impl Deserialize for Never {
    fn deserialize<D>(_: &mut D) -> Result<Self, D::Error> 
        where D: Deserializer
    {
        panic!("Never cannot be instantiated!");
    }
}

quick_error! {
    /// A `String` that impls `std::error::Error`.
    #[derive(Debug, Serialize, Deserialize)]
    pub enum StringError {
        /// The sole variant. Contains a `String`.
        Err(err: String) {
            from(err)
            description(err)
        }
    }
}
