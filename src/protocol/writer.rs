// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use byteorder::{BigEndian, ByteOrder};
use mio::{EventSet, Token, TryWrite};
use self::WriteState::*;
use std::collections::VecDeque;
use std::mem;
use std::io;
use std::rc::Rc;
use super::Packet;

/// Methods for writing bytes.
pub(super) trait Write: super::Len {
    /// Returns `true` iff the container is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slice the container starting from `from`.
    fn range_from(&self, from: usize) -> &[u8];
}

impl<D> Write for Rc<D>
    where D: Write
{
    #[inline]
    fn range_from(&self, from: usize) -> &[u8] {
        (&**self).range_from(from)
    }
}

impl Write for Vec<u8> {
    #[inline]
    fn range_from(&self, from: usize) -> &[u8] {
        &self[from..]
    }
}

impl Write for [u8; 8] {
    #[inline]
    fn range_from(&self, from: usize) -> &[u8] {
        &self[from..]
    }
}

#[derive(Debug)]
enum NextWriteAction {
    Stop,
    Continue,
}

#[derive(Debug)]
pub struct Writer<D> {
    written: usize,
    data: D,
}

impl<D> Writer<D> {
    /// Writes data to stream. Returns Ok(true) if all data has been written or Ok(false) if
    /// there's still data to write.
    fn try_write<W: TryWrite>(&mut self, stream: &mut W) -> io::Result<NextWriteAction>
        where D: Write
    {
        match try!(stream.try_write(&mut self.data.range_from(self.written))) {
            None => {
                debug!("Writer: spurious wakeup, {}/{}",
                       self.written,
                       self.data.len());
                Ok(NextWriteAction::Continue)
            }
            Some(bytes_written) => {
                debug!("Writer: wrote {} bytes of {} remaining.",
                       bytes_written,
                       self.data.len() - self.written);
                self.written += bytes_written;
                if self.written == self.data.len() {
                    Ok(NextWriteAction::Stop)
                } else {
                    Ok(NextWriteAction::Continue)
                }
            }
        }
    }
}

pub type U64Writer = Writer<[u8; 8]>;

impl U64Writer {
    fn empty() -> U64Writer {
        Writer {
            written: 0,
            data: [0; 8],
        }
    }

    fn from_u64(data: u64) -> Self {
        let mut buf = [0; 8];
        BigEndian::write_u64(&mut buf[..], data);

        Writer {
            written: 0,
            data: buf,
        }
    }
}

pub type VecWriter = Writer<Vec<u8>>;

/// A state machine that writes packets in non-blocking fashion.
#[derive(Debug)]
pub(super) enum WriteState<D> {
    WriteId {
        id: U64Writer,
        size: U64Writer,
        payload: Option<Writer<D>>,
    },
    WriteSize {
        size: U64Writer,
        payload: Option<Writer<D>>,
    },
    WriteData(Writer<D>),
}

#[derive(Debug)]
enum NextWriteState<D> {
    Same,
    Nothing,
    Next(WriteState<D>),
}

impl<D> WriteState<D> {
    pub(super) fn next<W: TryWrite>(state: &mut Option<WriteState<D>>,
                socket: &mut W,
                outbound: &mut VecDeque<Packet<D>>,
                interest: &mut EventSet,
                token: Token)
        where D: Write
    {
        let update = match *state {
            None => {
                match outbound.pop_front() {
                    Some(packet) => {
                        let size = packet.payload.len() as u64;
                        debug!("WriteState {:?}: Packet: id: {:?}, size: {}",
                               token,
                               packet.id,
                               size);
                        NextWriteState::Next(WriteState::WriteId {
                            id: U64Writer::from_u64(packet.id.0),
                            size: U64Writer::from_u64(size),
                            payload: if packet.payload.is_empty() {
                                None
                            } else {
                                Some(Writer {
                                    written: 0,
                                    data: packet.payload,
                                })
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
