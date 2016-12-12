// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {serde, tokio_core};
use bincode::{SizeLimit, serde as bincode};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::{self, Cursor};
use std::marker::PhantomData;
use std::mem;
use tokio_core::io::{EasyBuf, Framed, Io};
use tokio_proto::streaming::multiplex::{self, RequestId};
use tokio_proto::multiplex::{ClientProto, ServerProto};
use util::Never;

/// The type of message sent and received by the transport.
pub type Frame<T> = multiplex::Frame<T, Never, io::Error>;


// `T` is the type that `Codec` parses.
pub struct Codec<Req, Resp> {
    state: CodecState,
    _phantom_data: PhantomData<(Req, Resp)>,
}

enum CodecState {
    Id,
    Len { id: u64 },
    Payload { id: u64, len: u64 },
}

impl<Req, Resp> Codec<Req, Resp> {
    fn new() -> Self {
        Codec {
            state: CodecState::Id,
            _phantom_data: PhantomData,
        }
    }
}

impl<Req, Resp> tokio_core::io::Codec for Codec<Req, Resp>
    where Req: serde::Deserialize,
          Resp: serde::Serialize,
{
    type Out = (RequestId, Resp);
    type In = (RequestId, Result<Req, bincode::DeserializeError>);

    fn encode(&mut self, (id, message): Self::Out, buf: &mut Vec<u8>) -> io::Result<()> {
        buf.write_u64::<BigEndian>(id).unwrap();
        buf.write_u64::<BigEndian>(bincode::serialized_size(&message)).unwrap();
        bincode::serialize_into(buf,
                                &message,
                                SizeLimit::Infinite)
                 // TODO(tikue): handle err
                 .expect("In bincode::serialize_into");
        Ok(())
    }

    fn decode(&mut self, buf: &mut EasyBuf) -> Result<Option<Self::In>, io::Error> {
        use self::CodecState::*;

        loop {
            match self.state {
                Id if buf.len() < mem::size_of::<u64>() => return Ok(None),
                Id => {
                    let id_buf = buf.drain_to(mem::size_of::<u64>());
                    self.state = Len { id: Cursor::new(id_buf).read_u64::<BigEndian>()? };
                }
                Len { .. } if buf.len() < mem::size_of::<u64>() => return Ok(None),
                Len { id } => {
                    let len_buf = buf.drain_to(mem::size_of::<u64>());
                    self.state = Payload {
                        id: id,
                        len: Cursor::new(len_buf).read_u64::<BigEndian>()?,
                    };
                }
                Payload { len, .. } if buf.len() < len as usize => return Ok(None),
                Payload { id, .. } => {
                    let mut buf = buf.get_mut();
                    let result = bincode::deserialize_from(&mut Cursor::new(&mut *buf),
                                                           SizeLimit::Infinite);
                    // Clear any unread bytes so we don't read garbage on next request.
                    buf.clear();
                    // Reset the state machine because, either way, we're done processing this
                    // message.
                    self.state = Id;

                    return Ok(Some((id, result)));
                }
            }
        }
    }
}

/// Implements the `multiplex::ServerProto` trait.
pub struct Proto<Req, Resp>(PhantomData<(Req, Resp)>);

impl<Req, Resp> Proto<Req, Resp> {
    /// Returns a new `Proto`.
    pub fn new() -> Self {
        Proto(PhantomData)
    }
}

impl<T, Req, Resp> ServerProto<T> for Proto<Req, Resp>
    where T: Io + 'static,
          Req: serde::Deserialize + 'static,
          Resp: serde::Serialize + 'static,
{
    type Response = Resp;
    type Request = Result<Req, bincode::DeserializeError>;
    type Error = io::Error;
    type Transport = Framed<T, Codec<Req, Resp>>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(Codec::new()))
    }
}

impl<T, Req, Resp> ClientProto<T> for Proto<Req, Resp>
    where T: Io + 'static,
          Req: serde::Serialize + 'static,
          Resp: serde::Deserialize + 'static,
{
    type Response = Result<Resp, bincode::DeserializeError>;
    type Request = Req;
    type Error = io::Error;
    type Transport = Framed<T, Codec<Resp, Req>>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(Codec::new()))
    }
}

#[test]
fn serialize() {
    use tokio_core::io::Codec as TokioCodec;

    const MSG: (u64, (char, char, char)) = (4, ('a', 'b', 'c'));
    let mut buf = EasyBuf::new();
    let mut vec = Vec::new();

    // Serialize twice to check for idempotence.
    for _ in 0..2 {
        let mut codec: Codec<(char, char, char), (char, char, char)> = Codec::new();
        codec.encode(MSG, &mut vec).unwrap();
        buf.get_mut().append(&mut vec);
        let actual: Result<Option<(u64, Result<(char, char, char), bincode::DeserializeError>)>, io::Error> =
            codec.decode(&mut buf);

        match actual {
            Ok(Some((id, ref v))) if id == MSG.0 && *v.as_ref().unwrap() == MSG.1 => {}
            bad => panic!("Expected {:?}, but got {:?}", Some(MSG), bad),
        }

        assert!(buf.get_mut().is_empty(),
                "Expected empty buf but got {:?}",
                *buf.get_mut());
    }
}
