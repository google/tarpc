// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode;
use byteorder::{BigEndian, ReadBytesExt};
use bytes::BytesMut;
use bytes::buf::BufMut;
use serde;
use std::io::{self, Cursor};
use std::marker::PhantomData;
use std::mem;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::codec::{Encoder, Decoder, Framed};
use tokio_proto::multiplex::{ClientProto, ServerProto};
use tokio_proto::streaming::multiplex::RequestId;

// `Encode` is the type that `Codec` encodes. `Decode` is the type it decodes.
#[derive(Debug)]
pub struct Codec<Encode, Decode> {
    max_payload_size: u64,
    state: CodecState,
    _phantom_data: PhantomData<(Encode, Decode)>,
}

#[derive(Debug)]
enum CodecState {
    Id,
    Len { id: u64 },
    Payload { id: u64, len: u64 },
}

impl<Encode, Decode> Codec<Encode, Decode> {
    fn new(max_payload_size: u64) -> Self {
        Codec {
            max_payload_size: max_payload_size,
            state: CodecState::Id,
            _phantom_data: PhantomData,
        }
    }
}

fn too_big(payload_size: u64, max_payload_size: u64) -> io::Error {
    warn!(
        "Not sending too-big packet of size {} (max is {})",
        payload_size,
        max_payload_size
    );
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "Maximum payload size is {} bytes but got a payload of {}",
            max_payload_size,
            payload_size
        ),
    )
}

impl<Encode, Decode> Encoder for Codec<Encode, Decode>
where
    Encode: serde::Serialize,
    Decode: serde::de::DeserializeOwned,
{
    type Item = (RequestId, Encode);
    type Error = io::Error;

    fn encode(&mut self, (id, message): Self::Item, buf: &mut BytesMut) -> io::Result<()> {
        let payload_size = bincode::serialized_size(&message).map_err(|serialize_err| {
            io::Error::new(io::ErrorKind::Other, serialize_err)
        })?;
        if payload_size > self.max_payload_size {
            return Err(too_big(payload_size, self.max_payload_size));
        }
        let message_size = 2 * mem::size_of::<u64>() + payload_size as usize;
        buf.reserve(message_size);
        buf.put_u64::<BigEndian>(id);
        trace!("Encoded request id = {} as {:?}", id, buf);
        buf.put_u64::<BigEndian>(payload_size);
        bincode::serialize_into(&mut buf.writer(), &message)
            .map_err(|serialize_err| {
                io::Error::new(io::ErrorKind::Other, serialize_err)
            })?;
        trace!("Encoded buffer: {:?}", buf);
        Ok(())
    }
}

impl<Encode, Decode> Decoder for Codec<Encode, Decode>
where
    Decode: serde::de::DeserializeOwned,
{
    type Item = (RequestId, Result<Decode, bincode::Error>);
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Self::Item>> {
        use self::CodecState::*;
        trace!("Codec::decode: {:?}", buf);

        loop {
            match self.state {
                Id if buf.len() < mem::size_of::<u64>() => {
                    trace!("--> Buf len is {}; waiting for 8 to parse id.", buf.len());
                    return Ok(None);
                }
                Id => {
                    let mut id_buf = buf.split_to(mem::size_of::<u64>());
                    let id = Cursor::new(&mut id_buf).read_u64::<BigEndian>()?;
                    trace!("--> Parsed id = {} from {:?}", id, id_buf);
                    self.state = Len { id: id };
                }
                Len { .. } if buf.len() < mem::size_of::<u64>() => {
                    trace!(
                        "--> Buf len is {}; waiting for 8 to parse packet length.",
                        buf.len()
                    );
                    return Ok(None);
                }
                Len { id } => {
                    let len_buf = buf.split_to(mem::size_of::<u64>());
                    let len = Cursor::new(len_buf).read_u64::<BigEndian>()?;
                    trace!(
                        "--> Parsed payload length = {}, remaining buffer length = {}",
                        len,
                        buf.len()
                    );
                    if len > self.max_payload_size {
                        return Err(too_big(len, self.max_payload_size));
                    }
                    self.state = Payload { id: id, len: len };
                }
                Payload { len, .. } if buf.len() < len as usize => {
                    trace!(
                        "--> Buf len is {}; waiting for {} to parse payload.",
                        buf.len(),
                        len
                    );
                    return Ok(None);
                }
                Payload { id, len } => {
                    let payload = buf.split_to(len as usize);
                    let result = bincode::deserialize_from(&mut Cursor::new(payload));
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
#[derive(Debug)]
pub struct Proto<Encode, Decode> {
    max_payload_size: u64,
    _phantom_data: PhantomData<(Encode, Decode)>,
}

impl<Encode, Decode> Proto<Encode, Decode> {
    /// Returns a new `Proto`.
    pub fn new(max_payload_size: u64) -> Self {
        Proto {
            max_payload_size: max_payload_size,
            _phantom_data: PhantomData,
        }
    }
}

impl<T, Encode, Decode> ServerProto<T> for Proto<Encode, Decode>
where
    T: AsyncRead + AsyncWrite + 'static,
    Encode: serde::Serialize + 'static,
    Decode: serde::de::DeserializeOwned + 'static,
{
    type Response = Encode;
    type Request = Result<Decode, bincode::Error>;
    type Transport = Framed<T, Codec<Encode, Decode>>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(Codec::new(self.max_payload_size)))
    }
}

impl<T, Encode, Decode> ClientProto<T> for Proto<Encode, Decode>
where
    T: AsyncRead + AsyncWrite + 'static,
    Encode: serde::Serialize + 'static,
    Decode: serde::de::DeserializeOwned + 'static,
{
    type Response = Result<Decode, bincode::Error>;
    type Request = Encode;
    type Transport = Framed<T, Codec<Encode, Decode>>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(Codec::new(self.max_payload_size)))
    }
}

#[test]
fn serialize() {
    const MSG: (u64, (char, char, char)) = (4, ('a', 'b', 'c'));
    let mut buf = BytesMut::with_capacity(10);

    // Serialize twice to check for idempotence.
    for _ in 0..2 {
        let mut codec: Codec<(char, char, char), (char, char, char)> = Codec::new(2_000_000);
        codec.encode(MSG, &mut buf).unwrap();
        let actual: Result<
            Option<(u64, Result<(char, char, char), bincode::Error>)>,
            io::Error,
        > = codec.decode(&mut buf);

        match actual {
            Ok(Some((id, ref v))) if id == MSG.0 && *v.as_ref().unwrap() == MSG.1 => {}
            bad => panic!("Expected {:?}, but got {:?}", Some(MSG), bad),
        }

        assert!(buf.is_empty(), "Expected empty buf but got {:?}", buf);
    }
}

#[test]
fn deserialize_big() {
    let mut codec: Codec<Vec<u8>, Vec<u8>> = Codec::new(24);

    let mut buf = BytesMut::with_capacity(40);
    assert_eq!(
        codec
            .encode((0, vec![0; 24]), &mut buf)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::InvalidData
    );

    // Header
    buf.put_slice(&mut [0u8; 8]);
    // Len
    buf.put_slice(&mut [0u8, 0, 0, 0, 0, 0, 0, 25]);
    assert_eq!(
        codec.decode(&mut buf).err().unwrap().kind(),
        io::ErrorKind::InvalidData
    );
}
