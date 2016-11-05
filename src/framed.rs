// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::{SizeLimit, serde as bincode};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use futures::{Async, Poll};
use serde;
use std::io::{self, Cursor};
use std::marker::PhantomData;
use std::mem;
use tokio_core::easy::{self, EasyBuf, EasyFramed};
use tokio_core::io::{FramedIo, Io};
use tokio_proto::multiplex::{self, RequestId};
use util::Never;

/// Handles the IO of tarpc messages.
pub struct Framed<I, In, Out> {
    inner: EasyFramed<I, Parser<Out>, Serializer<In>>,
}

impl<I, In, Out> Framed<I, In, Out> {
    /// Constructs a new tarpc FramedIo
    pub fn new(upstream: I) -> Framed<I, In, Out>
        where I: Io,
              In: serde::Serialize,
              Out: serde::Deserialize
    {
        Framed { inner: EasyFramed::new(upstream, Parser::new(), Serializer::new()) }
    }
}

/// The type of message sent and received by the transport.
pub type Frame<T> = multiplex::Frame<T, Never, io::Error>;

impl<I, In, Out> FramedIo for Framed<I, In, Out>
    where I: Io,
          In: serde::Serialize,
          Out: serde::Deserialize
{
    type In = (RequestId, In);
    type Out = Option<(RequestId, Result<Out, bincode::DeserializeError>)>;

    fn poll_read(&mut self) -> Async<()> {
        self.inner.poll_read()
    }

    fn poll_write(&mut self) -> Async<()> {
        self.inner.poll_write()
    }

    fn read(&mut self) -> Poll<Self::Out, io::Error> {
        self.inner.read()
    }

    fn write(&mut self, req: Self::In) -> Poll<(), io::Error> {
        self.inner.write(req)
    }

    fn flush(&mut self) -> Poll<(), io::Error> {
        self.inner.flush()
    }
}

struct Parser<T> {
    state: ParserState,
    _phantom_data: PhantomData<T>,
}

enum ParserState {
    Id,
    Len { id: u64 },
    Payload { id: u64, len: u64 },
}

impl<T> Parser<T> {
    fn new() -> Self {
        Parser {
            state: ParserState::Id,
            _phantom_data: PhantomData,
        }
    }
}

impl<T> easy::Parse for Parser<T>
    where T: serde::Deserialize
{
    type Out = (RequestId, Result<T, bincode::DeserializeError>);

    fn parse(&mut self, buf: &mut EasyBuf) -> Poll<Self::Out, io::Error> {
        use self::ParserState::*;

        loop {
            match self.state {
                Id if buf.len() < mem::size_of::<u64>() => return Ok(Async::NotReady),
                Id => {
                    self.state = Len { id: Cursor::new(&*buf.get_mut()).read_u64::<BigEndian>()? };
                    *buf = buf.split_off(mem::size_of::<u64>());
                }
                Len { .. } if buf.len() < mem::size_of::<u64>() => return Ok(Async::NotReady),
                Len { id } => {
                    self.state = Payload {
                        id: id,
                        len: Cursor::new(&*buf.get_mut()).read_u64::<BigEndian>()?,
                    };
                    *buf = buf.split_off(mem::size_of::<u64>());
                }
                Payload { len, .. } if buf.len() < len as usize => return Ok(Async::NotReady),
                Payload { id, .. } => {
                    let mut buf = buf.get_mut();
                    let result = bincode::deserialize_from(&mut Cursor::new(&mut *buf),
                                                           SizeLimit::Infinite);
                    // Clear any unread bytes so we don't read garbage on next request.
                    buf.clear();
                    // Reset the state machine because, either way, we're done processing this
                    // message.
                    self.state = Id;

                    return Ok(Async::Ready((id, result)));
                }
            }
        }
    }
}

struct Serializer<T>(PhantomData<T>);

impl<T> Serializer<T> {
    fn new() -> Self {
        Serializer(PhantomData)
    }
}

impl<T> easy::Serialize for Serializer<T>
    where T: serde::Serialize
{
    type In = (RequestId, T);

    fn serialize(&mut self, (id, message): Self::In, buf: &mut Vec<u8>) {
        buf.write_u64::<BigEndian>(id).unwrap();
        buf.write_u64::<BigEndian>(bincode::serialized_size(&message)).unwrap();
        bincode::serialize_into(buf,
                                &message,
                                SizeLimit::Infinite)
                 // TODO(tikue): handle err
                 .expect("In bincode::serialize_into");
    }
}

#[test]
fn serialize() {
    use tokio_core::easy::{Parse, Serialize};

    const MSG: (u64, (char, char, char)) = (4, ('a', 'b', 'c'));
    let mut buf = EasyBuf::new();
    let mut vec = Vec::new();

    // Serialize twice to check for idempotence.
    for _ in 0..2 {
        Serializer::new().serialize(MSG, &mut vec);
        buf.get_mut().append(&mut vec);
        let actual: Poll<(u64, Result<(char, char, char), bincode::DeserializeError>), io::Error> =
            Parser::new().parse(&mut buf);

        match actual {
            Ok(Async::Ready((id, ref v))) if id == MSG.0 && *v.as_ref().unwrap() == MSG.1 => {}
            bad => panic!("Expected {:?}, but got {:?}", Some(MSG), bad),
        }

        assert!(buf.get_mut().is_empty(),
                "Expected empty buf but got {:?}",
                *buf.get_mut());
    }
}
