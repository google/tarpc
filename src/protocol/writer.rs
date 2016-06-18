// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use bytes::Buf;
use mio::{EventSet, Token, TryWrite};
use serde::Serialize;
use std::collections::VecDeque;
use std::mem;
use std::io::{self, Cursor};
use std::rc::Rc;
use super::RpcId;

/// The means of communication between client and server.
#[derive(Clone, Debug)]
pub struct Packet<B> {
    /// (id: u64, payload_len: u64, payload)
    ///
    /// The payload is typically a serialized message.
    pub buf: B,
}

pub trait WrappingBytes {
    fn wrapping(bytes: Vec<u8>) -> Self;
}

impl WrappingBytes for Cursor<Vec<u8>> {
    fn wrapping(bytes: Vec<u8>) -> Self {
        Cursor::new(bytes)
    }
}

impl<B> Packet<B>
    where B: Buf
{
    /// Overwrites the first 16 bytes of `payload` with (id, payload.len() - 16).
    pub fn overwriting_bytes(id: RpcId, mut payload: Vec<u8>) -> Packet<B>
        where B: WrappingBytes
    {
        (&mut payload[..8]).write_u64::<BigEndian>(id.0).unwrap();
        let len = payload.len() - 16;
        (&mut payload[8..16]).write_u64::<BigEndian>(len as u64).unwrap();
        Packet { buf: B::wrapping(payload) }
    }

    pub fn new<S>(id: RpcId, payload: &S) -> ::Result<Packet<B>>
        where S: Serialize,
              B: WrappingBytes
    {
        let payload_len = bincode::serialized_size(payload);
        let mut buf = Vec::with_capacity(2 * mem::size_of::<u64>() + payload_len as usize);
        buf.write_u64::<BigEndian>(id.0).unwrap();
        buf.write_u64::<BigEndian>(payload_len).unwrap();
        try!(bincode::serialize_into(&mut buf, payload, SizeLimit::Infinite));
        Ok(Packet { buf: B::wrapping(buf) })
    }

    pub fn empty(id: u64) -> Packet<B>
        where B: WrappingBytes
    {
        let mut buf = Vec::with_capacity(16);
        buf.write_u64::<BigEndian>(id).unwrap();
        buf.write_u64::<BigEndian>(0).unwrap();
        Packet { buf: B::wrapping(buf) }
    }

    #[inline]
    pub fn id(&self) -> RpcId {
        RpcId((&self.buf.bytes()[..8]).read_u64::<BigEndian>().unwrap())
    }

    #[inline]
    pub fn payload_len(&self) -> u64 {
        (&self.buf.bytes()[8..16]).read_u64::<BigEndian>().unwrap()
    }
}

#[derive(Debug)]
enum NextWriteAction {
    Stop,
    Continue,
}

trait BufExt: Buf + Sized {
    /// Writes data to stream. Returns Ok(true) if all data has been written or Ok(false) if
    /// there's still data to write.
    fn try_write<W: TryWrite>(&mut self, stream: &mut W) -> io::Result<NextWriteAction> {
        while let Some(bytes_written) = try!(stream.try_write_buf(self)) {
            debug!("Writer: wrote {} bytes; {} remaining.",
                   bytes_written,
                   self.remaining());
            if bytes_written == 0 {
                trace!("Writer: would block.");
                return Ok(NextWriteAction::Continue);
            }
            if !self.has_remaining() {
                return Ok(NextWriteAction::Stop);
            }
        }
        Ok(NextWriteAction::Continue)
    }
}

impl<B: Buf> BufExt for B {}

#[derive(Clone, Debug)]
pub struct RcBuf {
    written: usize,
    data: Rc<Vec<u8>>,
}

impl WrappingBytes for RcBuf {
    fn wrapping(bytes: Vec<u8>) -> Self {
        RcBuf {
            written: 0,
            data: Rc::new(bytes),
        }
    }
}

impl AsRef<[u8]> for RcBuf {
    fn as_ref(&self) -> &[u8] {
        self.data.as_ref()
    }
}

impl RcBuf {
    pub fn try_unwrap(self) -> Result<Vec<u8>, Self> {
        let written = self.written;
        Rc::try_unwrap(self.data).map_err(|rc| {
            RcBuf {
                written: written,
                data: rc,
            }
        })
    }
}

impl Buf for RcBuf {
    #[inline]
    fn remaining(&self) -> usize {
        self.data.len() - self.written
    }

    #[inline]
    fn bytes(&self) -> &[u8] {
        &self.data[self.written..]
    }

    #[inline]
    fn advance(&mut self, count: usize) {
        self.written += count;
    }
}

#[derive(Debug)]
pub enum NextWriteState<B> {
    Same,
    Nothing,
    Next(Packet<B>),
}

impl<B> NextWriteState<B> {
    pub fn next<W: TryWrite>(state: &mut Option<Packet<B>>,
                             socket: &mut W,
                             outbound: &mut VecDeque<Packet<B>>,
                             interest: &mut EventSet,
                             token: Token)
        where B: Buf
    {
        loop {
            let update = match *state {
                None => {
                    match outbound.pop_front() {
                        Some(packet) => {
                            let size = packet.buf.remaining() as u64;
                            debug_assert!(size >= mem::size_of::<u64>() as u64 * 2);
                            debug!("WriteState {:?}: Packet: id: {:?}, size: {}",
                                   token,
                                   packet.id(),
                                   size);
                            NextWriteState::Next(packet)
                        }
                        None => {
                            interest.remove(EventSet::writable());
                            NextWriteState::Same
                        }
                    }
                }
                Some(ref mut packet) => {
                    match packet.buf.try_write(socket) {
                        Ok(NextWriteAction::Stop) => {
                            debug!("WriteData {:?}: done writing payload", token);
                            NextWriteState::Nothing
                        }
                        Ok(NextWriteAction::Continue) => NextWriteState::Same,
                        Err(e) => {
                            debug!("WriteData {:?}: write err, {:?}", token, e);
                            NextWriteState::Nothing
                        }
                    }
                }
            };
            match update {
                NextWriteState::Next(next) => *state = Some(next),
                NextWriteState::Nothing => {
                    *state = None;
                    debug!("Remaining interests: {:?}", interest);
                }
                NextWriteState::Same => return,
            }
        }
    }
}
