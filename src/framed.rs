// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use serde;
use futures::Async;
use bincode::{SizeLimit, serde as bincode};
use byteorder::BigEndian;
use bytes::{Buf, MutBuf};
use bytes::buf::{BlockBuf, BlockBufCursor};
use std::{cmp, io, mem};
use std::marker::PhantomData;
use util::Never;
use tokio_core::io::{FramedIo, Io};
use tokio_proto::{self as proto, multiplex};

/// Handles the IO of tarpc messages.
pub struct Framed<I, In, Out> {
    inner: proto::Framed<I, Parser<Out>, Serializer<In>>,
}

impl<I, In, Out> Framed<I, In, Out> {
    /// Constructs a new tarpc FramedIo
    pub fn new(upstream: I) -> Framed<I, In, Out>
        where I: Io,
              In: serde::Serialize,
              Out: serde::Deserialize,
    {
        Framed {
            inner: proto::Framed::new(upstream,
                                      Parser::new(),
                                      Serializer::new(),
                                      BlockBuf::new(128, 8_192),
                                      BlockBuf::new(128, 8_192))
        }
    }

}

/// The type of message sent and received by the transport.
pub type Frame<T> = multiplex::Frame<T, Never, io::Error>;

impl<I, In, Out> FramedIo for Framed<I, In, Out>
    where I: Io,
          In: serde::Serialize,
          Out: serde::Deserialize,
{
    type In = Frame<In>;
    type Out = Frame<Result<Out, bincode::DeserializeError>>;

    fn poll_read(&mut self) -> Async<()> {
        self.inner.poll_read()
    }

    fn poll_write(&mut self) -> Async<()> {
        self.inner.poll_write()
    }

    fn read(&mut self) -> io::Result<Async<Self::Out>> {
        self.inner.read()
    }

    fn write(&mut self, req: Self::In) -> io::Result<Async<()>> {
        self.inner.write(req)
    }

    fn flush(&mut self) -> io::Result<Async<()>> {
        self.inner.flush()
    }
}

struct Parser<T> {
    state: ParserState,
    _phantom_data: PhantomData<T>
}

enum ParserState {
    Id,
    Len {
        id: u64,
    },
    Payload {
        id: u64,
        len: u64,
    }
}

impl<T> Parser<T> {
    fn new() -> Self {
        Parser {
            state: ParserState::Id,
            _phantom_data: PhantomData,
        }
    }
}

impl<T> proto::Parse for Parser<T>
    where T: serde::Deserialize,
{
    type Out = Frame<Result<T, bincode::DeserializeError>>;

    fn parse(&mut self, buf: &mut BlockBuf) -> Option<Self::Out> {
        use self::ParserState::*;

        loop {
            match self.state {
                Id if buf.len() < mem::size_of::<u64>() => return None,
                Id => {
                    self.state = Len {
                        id: buf.buf().read_u64::<BigEndian>()
                    };
                    buf.shift(mem::size_of::<u64>());
                }
                Len { .. } if buf.len() < mem::size_of::<u64>() => return None,
                Len { id }=> {
                    self.state = Payload {
                        id: id,
                        len: buf.buf().read_u64::<BigEndian>()
                    };
                    buf.shift(mem::size_of::<u64>());
                }
                Payload { len, .. } if buf.len() < len as usize => return None,
                Payload { id, len } => {
                    match bincode::deserialize_from(&mut BlockBufReader::new(buf),
                                                    SizeLimit::Infinite)
                    {
                        Ok(msg) => {
                            buf.shift(len as usize);
                            self.state = Id;
                            return Some(multiplex::Frame::Message(id, Ok(msg)));
                        }
                        Err(err) => {
                            // Clear any unread bytes so we don't read garbage on next request.
                            let buf_len = buf.len();
                            buf.shift(buf_len);
                            return Some(multiplex::Frame::Message(id, Err(err)));
                        }
                    }
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

impl<T> proto::Serialize for Serializer<T>
    where T: serde::Serialize,
{
    type In = Frame<T>;

    fn serialize(&mut self, msg: Self::In, buf: &mut BlockBuf) {
        use tokio_proto::multiplex::Frame::*;

        match msg {
            Message(id, msg) => {
                buf.write_u64::<BigEndian>(id);
                buf.write_u64::<BigEndian>(bincode::serialized_size(&msg));
                bincode::serialize_into(&mut BlockBufWriter::new(buf),
                                        &msg,
                                        SizeLimit::Infinite)
                         // TODO(tikue): handle err
                         .expect("In bincode::serialize_into");
            }
            Error(id, e) => panic!("Unexpected error in Serializer::serialize, id={}: {}", id, e),
            MessageWithBody(..) | Body(..) | Done => unreachable!(),
        }
        
    }
}

// == Scaffolding from Buf/MutBuf to Read/Write ==

struct BlockBufReader<'a> {
    cursor: BlockBufCursor<'a>,
}

impl<'a> BlockBufReader<'a> {
    fn new(buf: &'a mut BlockBuf) -> Self {
        BlockBufReader { cursor: buf.buf() }
    }
}

impl<'a> io::Read for BlockBufReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let init_remaining = self.cursor.remaining();
        let buf_len = buf.len();
        self.cursor.read_slice(&mut buf[..cmp::min(init_remaining, buf_len)]);
        Ok(init_remaining - self.cursor.remaining())
    }
}

struct BlockBufWriter<'a> {
    buf: &'a mut BlockBuf,
}

impl<'a> BlockBufWriter<'a> {
    fn new(buf: &'a mut BlockBuf) -> Self {
        BlockBufWriter { buf: buf }
    }
}

impl<'a> io::Write for BlockBufWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.write_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Always writes immediately, so there's never anything to flush.
        Ok(())
    }
}

#[test]
fn serialize() {
    use tokio_proto::{Parse, Serialize};

    const MSG: Frame<(char, char, char)> = multiplex::Frame::Message(4, ('a', 'b', 'c'));
    let mut buf = BlockBuf::default();

    // Serialize twice to check for idempotence.
    for _ in 0..2 {
        Serializer::new().serialize(MSG, &mut buf);
        let actual: Option<Frame<Result<(char, char, char), bincode::DeserializeError>>> = Parser::new().parse(&mut buf);

        match actual {
            Some(multiplex::Frame::Message(id, ref v)) if id == MSG.request_id().unwrap() && 
                *v.as_ref().unwrap() == MSG.unwrap_msg() => {} // good,
            bad => panic!("Expected {:?}, but got {:?}", Some(MSG), bad),
        }

        assert!(buf.is_empty(),
                "Expected empty buf but got {:?}",
                {buf.compact(); buf.bytes().unwrap()});
    }
}
