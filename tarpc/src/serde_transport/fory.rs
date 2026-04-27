// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! TCP transport using Apache Fory codec.
//!
//! Mirrors [`crate::serde_transport::tcp`] but serializes tarpc envelope types
//! via [`fory`] instead of JSON/bincode. Requires the `serde-transport-fory`
//! and `tcp` features.
//!
//! # Quick start
//!
//! ```no_run
//! # #[cfg(all(feature = "serde-transport-fory", feature = "tcp"))]
//! # async fn example() -> std::io::Result<()> {
//! use fory::Fory;
//! use std::sync::Arc;
//! use tarpc::serde_transport::fory as fory_transport;
//!
//! let fory = Arc::new(Fory::default());
//!
//! // Server side
//! let incoming = fory_transport::listen::<_, String, String>("127.0.0.1:0", fory.clone()).await?;
//! let addr = incoming.local_addr();
//!
//! // Client side
//! let transport = fory_transport::connect::<_, String, String>(addr, fory).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Type registration
//!
//! The fory codec requires all types that cross the wire to be registered in the
//! same [`fory::Fory`] instance on both sides with identical numeric IDs. At a
//! minimum register the envelope wrapper types from
//! [`crate::serde_transport::fory_envelope`] **plus** your own `Req`/`Resp` types
//! (unless they are built-in primitive types that fory handles natively).

use super::fory_envelope::{ForyClientMessage, ForyResponse};
use crate::{ClientMessage, Response};
use fory::Fory;
use std::{io, marker::PhantomData, pin::Pin, sync::Arc};
use tokio_util::bytes::{Bytes, BytesMut};

// The parent module (serde_transport) exposes `new` and `Transport`.
use crate::serde_transport::{self, Transport};

// ---------------------------------------------------------------------------
// ForyEnvelopeCodec
// ---------------------------------------------------------------------------

/// Codec that converts tarpc's native envelope types ↔ fory wrapper types at
/// the wire boundary.
///
/// `Req` is the user-defined request message type; `Resp` is the response
/// message type. The codec works symmetrically on both client and server sides:
///
/// - **Client side**: `Serializer<ClientMessage<Req>>` + `Deserializer<Response<Resp>>`
/// - **Server side**: `Serializer<Response<Resp>>` + `Deserializer<ClientMessage<Req>>`
pub struct ForyEnvelopeCodec<Req, Resp> {
    fory: Arc<Fory>,
    _marker: PhantomData<(Req, Resp)>,
}

impl<Req, Resp> ForyEnvelopeCodec<Req, Resp> {
    /// Create a new codec sharing the given [`Fory`] registry.
    pub fn new(fory: Arc<Fory>) -> Self {
        Self { fory, _marker: PhantomData }
    }
}

impl<Req, Resp> Clone for ForyEnvelopeCodec<Req, Resp> {
    fn clone(&self) -> Self {
        Self { fory: self.fory.clone(), _marker: PhantomData }
    }
}

// ---------------------------------------------------------------------------
// Client side: send ClientMessage<Req>
// ---------------------------------------------------------------------------

impl<Req, Resp> tokio_serde::Serializer<ClientMessage<Req>> for ForyEnvelopeCodec<Req, Resp>
where
    Req: fory::Serializer + fory::ForyDefault + Clone + Send + 'static,
{
    type Error = io::Error;

    fn serialize(
        self: Pin<&mut Self>,
        item: &ClientMessage<Req>,
    ) -> Result<Bytes, Self::Error> {
        let wrapper = ForyClientMessage::<Req>::from(item);
        self.fory
            .serialize(&wrapper)
            .map(Bytes::from)
            .map_err(|e: fory::Error| {
                io::Error::new(io::ErrorKind::InvalidData, e.to_string())
            })
    }
}

// ---------------------------------------------------------------------------
// Client side: receive Response<Resp>
// ---------------------------------------------------------------------------

impl<Req, Resp> tokio_serde::Deserializer<Response<Resp>> for ForyEnvelopeCodec<Req, Resp>
where
    Resp: fory::Serializer + fory::ForyDefault + Send + 'static,
{
    type Error = io::Error;

    fn deserialize(
        self: Pin<&mut Self>,
        src: &BytesMut,
    ) -> Result<Response<Resp>, Self::Error> {
        let wrapper: ForyResponse<Resp> = self
            .fory
            .deserialize(src.as_ref())
            .map_err(|e: fory::Error| {
                io::Error::new(io::ErrorKind::InvalidData, e.to_string())
            })?;
        Ok(Response::from(wrapper))
    }
}

// ---------------------------------------------------------------------------
// Server side: send Response<Resp>
// ---------------------------------------------------------------------------

impl<Req, Resp> tokio_serde::Serializer<Response<Resp>> for ForyEnvelopeCodec<Req, Resp>
where
    Resp: fory::Serializer + fory::ForyDefault + Clone + Send + 'static,
{
    type Error = io::Error;

    fn serialize(
        self: Pin<&mut Self>,
        item: &Response<Resp>,
    ) -> Result<Bytes, Self::Error> {
        let wrapper = ForyResponse::<Resp>::from(item);
        self.fory
            .serialize(&wrapper)
            .map(Bytes::from)
            .map_err(|e: fory::Error| {
                io::Error::new(io::ErrorKind::InvalidData, e.to_string())
            })
    }
}

// ---------------------------------------------------------------------------
// Server side: receive ClientMessage<Req>
// ---------------------------------------------------------------------------

impl<Req, Resp> tokio_serde::Deserializer<ClientMessage<Req>> for ForyEnvelopeCodec<Req, Resp>
where
    Req: fory::Serializer + fory::ForyDefault + Send + 'static,
{
    type Error = io::Error;

    fn deserialize(
        self: Pin<&mut Self>,
        src: &BytesMut,
    ) -> Result<ClientMessage<Req>, Self::Error> {
        let wrapper: ForyClientMessage<Req> = self
            .fory
            .deserialize(src.as_ref())
            .map_err(|e: fory::Error| {
                io::Error::new(io::ErrorKind::InvalidData, e.to_string())
            })?;
        Ok(ClientMessage::from(wrapper))
    }
}

// ---------------------------------------------------------------------------
// TCP support (requires `tcp` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "tcp")]
pub use tcp::{Incoming, connect, listen};

#[cfg(feature = "tcp")]
mod tcp {
    use super::*;
    use futures::ready;
    use pin_project::pin_project;
    use std::net::SocketAddr;
    use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
    use tokio_util::codec::length_delimited;

    // -----------------------------------------------------------------------
    // connect
    // -----------------------------------------------------------------------

    /// Connects to `addr` and returns a client-side fory transport.
    ///
    /// The transport sinks `ClientMessage<Req>` and streams `Response<Resp>`.
    pub async fn connect<A, Req, Resp>(
        addr: A,
        fory: Arc<Fory>,
    ) -> io::Result<
        Transport<TcpStream, Response<Resp>, ClientMessage<Req>, ForyEnvelopeCodec<Req, Resp>>,
    >
    where
        A: ToSocketAddrs,
        Req: fory::Serializer + fory::ForyDefault + Clone + Send + 'static,
        Resp: fory::Serializer + fory::ForyDefault + Send + 'static,
        // serde bounds required by Transport's Stream/Sink impls
        ClientMessage<Req>: serde::Serialize,
        Response<Resp>: for<'de> serde::Deserialize<'de>,
    {
        let stream = TcpStream::connect(addr).await?;
        let framed = length_delimited::Builder::new()
            .max_frame_length(usize::MAX / 2)
            .new_framed(stream);
        Ok(serde_transport::new(framed, ForyEnvelopeCodec::new(fory)))
    }

    // -----------------------------------------------------------------------
    // listen / Incoming
    // -----------------------------------------------------------------------

    /// A [`TcpListener`] that wraps accepted connections in fory-encoded
    /// server-side transports.
    #[pin_project]
    pub struct Incoming<Req, Resp> {
        #[pin]
        listener: TcpListener,
        fory: Arc<Fory>,
        local_addr: SocketAddr,
        config: length_delimited::Builder,
        _marker: PhantomData<(Req, Resp)>,
    }

    impl<Req, Resp> Incoming<Req, Resp> {
        /// Returns the address being listened on.
        pub fn local_addr(&self) -> SocketAddr {
            self.local_addr
        }

        /// Returns an immutable reference to the length-delimited codec's config.
        pub fn config(&self) -> &length_delimited::Builder {
            &self.config
        }

        /// Returns a mutable reference to the length-delimited codec's config.
        pub fn config_mut(&mut self) -> &mut length_delimited::Builder {
            &mut self.config
        }
    }

    /// Listens on `addr` and returns an [`Incoming`] stream of server-side
    /// transports.
    ///
    /// Each accepted transport sinks `Response<Resp>` and streams
    /// `ClientMessage<Req>`.
    pub async fn listen<A, Req, Resp>(addr: A, fory: Arc<Fory>) -> io::Result<Incoming<Req, Resp>>
    where
        A: ToSocketAddrs,
    {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(Incoming {
            listener,
            fory,
            local_addr,
            config: *length_delimited::Builder::new().max_frame_length(usize::MAX / 2),
            _marker: PhantomData,
        })
    }

    impl<Req, Resp> futures::stream::Stream for Incoming<Req, Resp>
    where
        Req: fory::Serializer + fory::ForyDefault + Send + 'static,
        Resp: fory::Serializer + fory::ForyDefault + Clone + Send + 'static,
        // serde bounds required by Transport's Stream/Sink impls
        Response<Resp>: serde::Serialize,
        ClientMessage<Req>: for<'de> serde::Deserialize<'de>,
    {
        type Item = io::Result<
            Transport<
                TcpStream,
                ClientMessage<Req>,
                Response<Resp>,
                ForyEnvelopeCodec<Req, Resp>,
            >,
        >;

        fn poll_next(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            let accept_result = ready!(self.as_mut().project().listener.poll_accept(cx));
            let (stream, _peer) = match accept_result {
                Ok(pair) => pair,
                Err(e) => return std::task::Poll::Ready(Some(Err(e))),
            };
            let framed = self.config.new_framed(stream);
            let transport = serde_transport::new(
                framed,
                ForyEnvelopeCodec::<Req, Resp>::new(self.fory.clone()),
            );
            std::task::Poll::Ready(Some(Ok(transport)))
        }
    }
}
