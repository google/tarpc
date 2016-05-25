// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use byteorder::{BigEndian, ReadBytesExt};
use mio::{Token, TryRead};
use self::ReadState::*;
use std::io;
use std::mem;
use super::RpcId;

/// Methods for reading bytes.
pub(super) trait Read: super::Len {
/// The resulting type once all bytes are read.
    type Read;

/// Mutably slice the container starting from `from`.
    fn range_from_mut(&mut self, from: usize) -> &mut [u8];

/// Read the bytes into a type.
    fn read(&mut self) -> Self::Read;
}

impl Read for Vec<u8> {
    type Read = Self;

    #[inline]
    fn range_from_mut(&mut self, from: usize) -> &mut [u8] {
        &mut self[from..]
    }

    #[inline]
    fn read(&mut self) -> Self {
        mem::replace(self, vec![])
    }
}

impl Read for [u8; 8] {
    type Read = u64;

    #[inline]
    fn range_from_mut(&mut self, from: usize) -> &mut [u8] {
        &mut self[from..]
    }

    #[inline]
    fn read(&mut self) -> u64 {
        (self as &[u8]).read_u64::<BigEndian>().unwrap()
    }
}

type Packet = super::Packet<Vec<u8>>;

#[derive(Debug)]
pub struct Reader<D> {
    read: usize,
    data: D,
}

#[derive(Debug)]
enum NextReadAction<D>
    where D: Read
{
    Continue,
    Stop(D::Read),
}

impl<D> Reader<D> {
    fn try_read<R: TryRead>(&mut self, stream: &mut R) -> io::Result<NextReadAction<D>>
        where D: Read
    {
        match try!(stream.try_read(self.data.range_from_mut(self.read))) {
            None => {
                debug!("Reader: spurious wakeup, {}/{}", self.read, self.data.len());
                Ok(NextReadAction::Continue)
            }
            Some(bytes_read) => {
                debug!("Reader: read {} bytes of {} remaining.",
                       bytes_read,
                       self.data.len() - self.read);
                self.read += bytes_read;
                if self.read == self.data.len() {
                    trace!("Reader: finished.");
                    Ok(NextReadAction::Stop(self.data.read()))
                } else {
                    trace!("Reader: not finished.");
                    Ok(NextReadAction::Continue)
                }
            }
        }
    }
}

pub type U64Reader = Reader<[u8; 8]>;

impl U64Reader {
    fn new() -> Self {
        Reader {
            read: 0,
            data: [0; 8],
        }
    }
}

pub type VecReader = Reader<Vec<u8>>;

impl VecReader {
    fn with_len(len: usize) -> Self {
        VecReader {
            read: 0,
            data: vec![0; len],
        }
    }
}

/// A state machine that reads packets in non-blocking fashion.
#[derive(Debug)]
pub enum ReadState {
    /// Tracks how many bytes of the message ID have been read.
    ReadId(U64Reader),
    /// Tracks how many bytes of the message size have been read.
    ReadLen {
        id: u64,
        len: U64Reader,
    },
    /// Tracks read progress.
    ReadData {
        /// ID of the message being read.
        id: u64,
        /// Reads the bufer.
        buf: VecReader,
    },
}

#[derive(Debug)]
enum NextReadState {
    Same,
    Next(ReadState),
    Reset(Packet),
}

impl ReadState {
    pub fn init() -> ReadState {
        ReadId(U64Reader::new())
    }

    pub fn next<R: TryRead>(state: &mut ReadState,
                            socket: &mut R,
                            token: Token)
                            -> Option<super::Packet<Vec<u8>>> {
        let next = match *state {
            ReadId(ref mut reader) => {
                debug!("ReadState {:?}: reading id.", token);
                match reader.try_read(socket) {
                    Ok(NextReadAction::Continue) => NextReadState::Same,
                    Ok(NextReadAction::Stop(id)) => {
                        debug!("ReadId {:?}: transitioning to reading len.", token);
                        NextReadState::Next(ReadLen {
                            id: id,
                            len: U64Reader::new(),
                        })
                    }
                    Err(e) => {
                        // TODO(tikue): handle this better?
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        NextReadState::Same
                    }
                }
            }
            ReadLen { id, ref mut len } => {
                match len.try_read(socket) {
                    Ok(NextReadAction::Continue) => NextReadState::Same,
                    Ok(NextReadAction::Stop(len)) => {
                        debug!("ReadLen: message len = {}", len);
                        if len == 0 {
                            debug!("Reading complete.");
                            NextReadState::Reset(Packet {
                                id: RpcId(id),
                                payload: vec![],
                            })
                        } else {
                            debug!("ReadLen {:?}: transitioning to reading payload.", token);
                            NextReadState::Next(ReadData {
                                id: id,
                                buf: VecReader::with_len(len as usize),
                            })
                        }
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        NextReadState::Same
                    }
                }
            }
            ReadData { id, ref mut buf } => {
                match buf.try_read(socket) {
                    Ok(NextReadAction::Continue) => NextReadState::Same,
                    Ok(NextReadAction::Stop(payload)) => {
                        NextReadState::Reset(Packet {
                            id: RpcId(id),
                            payload: payload,
                        })
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        NextReadState::Same
                    }
                }
            }
        };
        match next {
            NextReadState::Same => None,
            NextReadState::Next(next) => {
                *state = next;
                None
            }
            NextReadState::Reset(packet) => {
                *state = ReadState::init();
                Some(packet)
            }
        }
    }
}
