//! `Stream` and `Sink` adaptors for serializing and deserializing values using
//! Bincode.
//!
//! This crate provides adaptors for going from a stream or sink of buffers
//! ([`Bytes`]) to a stream or sink of values by performing Bincode encoding or
//! decoding. It is expected that each yielded buffer contains a single
//! serialized Bincode value. The specific strategy by which this is done is left
//! up to the user. One option is to use using [`length_delimited`] from
//! [tokio-io].
//!
//! [`Bytes`]: https://docs.rs/bytes/0.4/bytes/struct.Bytes.html
//! [`length_delimited`]: http://alexcrichton.com/tokio-io/tokio_io/codec/length_delimited/index.html
//! [tokio-io]: http://github.com/alexcrichton/tokio-io
//! [examples]: https://github.com/carllerche/tokio-serde-json/tree/master/examples

use bincode::Error;
use bytes::{Bytes, BytesMut};
use futures_legacy::{Poll, Sink, StartSend, Stream};
use serde::{Deserialize, Serialize};
use std::io;
use tokio_serde::{Deserializer, FramedRead, FramedWrite, Serializer};

use std::marker::PhantomData;

/// Adapts a stream of Bincode encoded buffers to a stream of values by
/// deserializing them.
///
/// `ReadBincode` implements `Stream` by polling the inner buffer stream and
/// deserializing the buffer as Bincode. It expects that each yielded buffer
/// represents a single Bincode value and does not contain any extra trailing
/// bytes.
crate struct ReadBincode<T, U> {
    inner: FramedRead<T, U, Bincode<U>>,
}

/// Adapts a buffer sink to a value sink by serializing the values as Bincode.
///
/// `WriteBincode` implements `Sink` by serializing the submitted values to a
/// buffer. The buffer is then sent to the inner stream, which is responsible
/// for handling framing on the wire.
crate struct WriteBincode<T: Sink, U> {
    inner: FramedWrite<T, U, Bincode<U>>,
}

struct Bincode<T> {
    ghost: PhantomData<T>,
}

impl<T, U> ReadBincode<T, U>
where
    T: Stream<Error = IoErrorWrapper>,
    U: for<'de> Deserialize<'de>,
    Bytes: From<T::Item>,
{
    /// Creates a new `ReadBincode` with the given buffer stream.
    pub fn new(inner: T) -> ReadBincode<T, U> {
        let json = Bincode { ghost: PhantomData };
        ReadBincode {
            inner: FramedRead::new(inner, json),
        }
    }
}

impl<T, U> ReadBincode<T, U> {
    /// Returns a mutable reference to the underlying stream wrapped by
    /// `ReadBincode`.
    ///
    /// Note that care should be taken to not tamper with the underlying stream
    /// of data coming in as it may corrupt the stream of frames otherwise
    /// being worked with.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }
}

impl<T, U> Stream for ReadBincode<T, U>
where
    T: Stream<Error = IoErrorWrapper>,
    U: for<'de> Deserialize<'de>,
    Bytes: From<T::Item>,
{
    type Item = U;
    type Error = <T as Stream>::Error;

    fn poll(&mut self) -> Poll<Option<U>, Self::Error> {
        self.inner.poll()
    }
}

impl<T, U> Sink for ReadBincode<T, U>
where
    T: Sink,
{
    type SinkItem = T::SinkItem;
    type SinkError = T::SinkError;

    fn start_send(&mut self, item: T::SinkItem) -> StartSend<T::SinkItem, T::SinkError> {
        self.get_mut().start_send(item)
    }

    fn poll_complete(&mut self) -> Poll<(), T::SinkError> {
        self.get_mut().poll_complete()
    }

    fn close(&mut self) -> Poll<(), T::SinkError> {
        self.get_mut().close()
    }
}

crate struct IoErrorWrapper(pub io::Error);
impl From<Box<bincode::ErrorKind>> for IoErrorWrapper {
    fn from(e: Box<bincode::ErrorKind>) -> Self {
        IoErrorWrapper(match *e {
            bincode::ErrorKind::Io(e) => e,
            bincode::ErrorKind::InvalidUtf8Encoding(e) => {
                io::Error::new(io::ErrorKind::InvalidInput, e)
            }
            bincode::ErrorKind::InvalidBoolEncoding(e) => {
                io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
            }
            bincode::ErrorKind::InvalidTagEncoding(e) => {
                io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
            }
            bincode::ErrorKind::InvalidCharEncoding => {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid char encoding")
            }
            bincode::ErrorKind::DeserializeAnyNotSupported => {
                io::Error::new(io::ErrorKind::InvalidInput, "Deserialize Any not supported")
            }
            bincode::ErrorKind::SizeLimit => {
                io::Error::new(io::ErrorKind::InvalidInput, "Size limit exceeded")
            }
            bincode::ErrorKind::SequenceMustHaveLength => {
                io::Error::new(io::ErrorKind::InvalidInput, "Sequence must have length")
            }
            bincode::ErrorKind::Custom(s) => io::Error::new(io::ErrorKind::Other, s),
        })
    }
}

impl From<IoErrorWrapper> for io::Error {
    fn from(wrapper: IoErrorWrapper) -> io::Error {
        wrapper.0
    }
}

impl<T, U> WriteBincode<T, U>
where
    T: Sink<SinkItem = BytesMut, SinkError = IoErrorWrapper>,
    U: Serialize,
{
    /// Creates a new `WriteBincode` with the given buffer sink.
    pub fn new(inner: T) -> WriteBincode<T, U> {
        let json = Bincode { ghost: PhantomData };
        WriteBincode {
            inner: FramedWrite::new(inner, json),
        }
    }
}

impl<T: Sink, U> WriteBincode<T, U> {
    /// Returns a mutable reference to the underlying sink wrapped by
    /// `WriteBincode`.
    ///
    /// Note that care should be taken to not tamper with the underlying sink as
    /// it may corrupt the sequence of frames otherwise being worked with.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }
}

impl<T, U> Sink for WriteBincode<T, U>
where
    T: Sink<SinkItem = BytesMut, SinkError = IoErrorWrapper>,
    U: Serialize,
{
    type SinkItem = U;
    type SinkError = <T as Sink>::SinkError;

    fn start_send(&mut self, item: U) -> StartSend<U, Self::SinkError> {
        self.inner.start_send(item)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.poll_complete()
    }

    fn close(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.poll_complete()
    }
}

impl<T, U> Stream for WriteBincode<T, U>
where
    T: Stream + Sink,
{
    type Item = T::Item;
    type Error = T::Error;

    fn poll(&mut self) -> Poll<Option<T::Item>, T::Error> {
        self.get_mut().poll()
    }
}

impl<T> Deserializer<T> for Bincode<T>
where
    T: for<'de> Deserialize<'de>,
{
    type Error = Error;

    fn deserialize(&mut self, src: &Bytes) -> Result<T, Error> {
        bincode::deserialize(src)
    }
}

impl<T: Serialize> Serializer<T> for Bincode<T> {
    type Error = Error;

    fn serialize(&mut self, item: &T) -> Result<BytesMut, Self::Error> {
        bincode::serialize(item).map(Into::into)
    }
}
