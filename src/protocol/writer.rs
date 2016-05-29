// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use byteorder::{BigEndian, ByteOrder};
use bytes::Buf;
use mio::{EventSet, Token, TryWrite};
use self::WriteState::*;
use std::collections::VecDeque;
use std::mem;
use std::io;
use std::rc::Rc;
use super::Packet;

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
        debug!("Writer: spurious wakeup; {} remaining", self.remaining());
        Ok(NextWriteAction::Continue)
    }
}

impl<B: Buf> BufExt for B {}

#[derive(Debug)]
pub struct U64Writer {
    written: usize,
    data: [u8; 8],
}

impl U64Writer {
    #[inline]
    fn empty() -> Self {
        U64Writer {
            written: 0,
            data: [0; 8],
        }
    }

    #[inline]
    fn from_u64(data: u64) -> Self {
        let mut buf = [0; 8];
        BigEndian::write_u64(&mut buf[..], data);

        U64Writer {
            written: 0,
            data: buf,
        }
    }
}

impl Buf for U64Writer {
    #[inline]
    fn remaining(&self) -> usize {
        8 - self.written
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

#[derive(Clone, Debug)]
pub struct RcBuf {
    written: usize,
    data: Rc<Vec<u8>>,
}

impl RcBuf {
    pub fn from_vec(buf: Vec<u8>) -> Self {
        RcBuf {
            written: 0,
            data: Rc::new(buf),
        }
    }

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

/// A state machine that writes packets in non-blocking fashion.
#[derive(Debug)]
pub(super) enum WriteState<B> {
    WriteId {
        id: U64Writer,
        size: U64Writer,
        payload: Option<B>,
    },
    WriteSize { size: U64Writer, payload: Option<B> },
    WriteData(B),
}

#[derive(Debug)]
enum NextWriteState<B> {
    Same,
    Nothing,
    Next(WriteState<B>),
}

impl<B> WriteState<B> {
    pub fn next<W: TryWrite>(state: &mut Option<WriteState<B>>,
                             socket: &mut W,
                             outbound: &mut VecDeque<Packet<B>>,
                             interest: &mut EventSet,
                             token: Token)
        where B: Buf
    {
        let update = match *state {
            None => {
                match outbound.pop_front() {
                    Some(packet) => {
                        let size = packet.payload.remaining() as u64;
                        debug!("WriteState {:?}: Packet: id: {:?}, size: {}",
                               token,
                               packet.id,
                               size);
                        NextWriteState::Next(WriteState::WriteId {
                            id: U64Writer::from_u64(packet.id.0),
                            size: U64Writer::from_u64(size),
                            payload: if packet.payload.has_remaining() {
                                Some(packet.payload)
                            } else {
                                None
                            },
                        })
                    }
                    None => {
                        interest.remove(EventSet::writable());
                        NextWriteState::Same
                    }
                }
            }
            Some(WriteId { ref mut id, ref mut size, ref mut payload }) => {
                match id.try_write(socket) {
                    Ok(NextWriteAction::Stop) => {
                        debug!("WriteId {:?}: transitioning to writing size", token);
                        let size = mem::replace(size, U64Writer::empty());
                        let payload = mem::replace(payload, None);
                        NextWriteState::Next(WriteState::WriteSize {
                            size: size,
                            payload: payload,
                        })
                    }
                    Ok(NextWriteAction::Continue) => NextWriteState::Same,
                    Err(e) => {
                        debug!("WriteId {:?}: write err, {:?}", token, e);
                        NextWriteState::Nothing
                    }
                }
            }
            Some(WriteSize { ref mut size, ref mut payload }) => {
                match size.try_write(socket) {
                    Ok(NextWriteAction::Stop) => {
                        let payload = mem::replace(payload, None);
                        if let Some(payload) = payload {
                            debug!("WriteSize {:?}: Transitioning to writing payload", token);
                            NextWriteState::Next(WriteState::WriteData(payload))
                        } else {
                            debug!("WriteSize {:?}: no payload to write.", token);
                            NextWriteState::Nothing
                        }
                    }
                    Ok(NextWriteAction::Continue) => NextWriteState::Same,
                    Err(e) => {
                        debug!("WriteSize {:?}: write err, {:?}", token, e);
                        NextWriteState::Nothing
                    }
                }
            }
            Some(WriteData(ref mut payload)) => {
                match payload.try_write(socket) {
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
                debug!("WriteSize {:?}: Done writing.", token);
                if outbound.is_empty() {
                    interest.remove(EventSet::writable());
                }
                debug!("Remaining interests: {:?}", interest);
            }
            NextWriteState::Same => {}
        }
    }
}
