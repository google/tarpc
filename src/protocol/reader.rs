// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use byteorder::{BigEndian, ReadBytesExt};
use bytes::{MutBuf, Take};
use mio::{Token, TryRead};
use self::ReadState::*;
use std::io;
use std::mem;
use super::RpcId;

type Packet = super::Packet<Vec<u8>>;

#[derive(Debug)]
pub struct U64Reader {
    read: usize,
    data: [u8; 8],
}

impl U64Reader {
    fn new() -> Self {
        U64Reader {
            read: 0,
            data: [0; 8],
        }
    }
}

impl MutBuf for U64Reader {
    fn remaining(&self) -> usize {
        8 - self.read
    }

    unsafe fn advance(&mut self, count: usize) {
        self.read += count;
    }

    unsafe fn mut_bytes(&mut self) -> &mut [u8] {
        &mut self.data[self.read..]
    }
}

#[derive(Debug)]
enum NextReadAction<R> {
    Continue,
    Stop(R),
}

trait MutBufExt: MutBuf {
    type Inner;

    fn take(&mut self) -> Self::Inner;

    fn try_read<R: TryRead>(&mut self, stream: &mut R) -> io::Result<NextReadAction<Self::Inner>> {
        while let Some(bytes_read) = try!(stream.try_read_buf(self)) {
            debug!("Reader: read {} bytes, {} remaining.",
                   bytes_read,
                   self.remaining());
            if bytes_read == 0 {
                trace!("Reader: would block.");
                return Ok(NextReadAction::Continue);
            }

            if !self.has_remaining() {
                trace!("Reader: finished.");
                return Ok(NextReadAction::Stop(self.take()));
            }
        }
        debug!("Reader: spurious wakeup; {} remaining", self.remaining());
        Ok(NextReadAction::Continue)
    }
}

impl MutBufExt for U64Reader {
    type Inner = u64;

    fn take(&mut self) -> u64 {
        (&self.data as &[u8]).read_u64::<BigEndian>().unwrap()
    }
}

impl MutBufExt for Take<Vec<u8>> {
    type Inner = Vec<u8>;

    fn take(&mut self) -> Vec<u8> {
        mem::replace(self.get_mut(), vec![])
    }
}

/// A state machine that reads packets in non-blocking fashion.
#[derive(Debug)]
pub enum ReadState {
    /// Tracks how many bytes of the message ID have been read.
    ReadId(U64Reader),
    /// Tracks how many bytes of the message size have been read.
    ReadLen { id: u64, len: U64Reader },
    /// Tracks read progress.
    ReadData {
        /// ID of the message being read.
        id: u64,
        /// Reads the bufer.
        buf: Take<Vec<u8>>,
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

    pub fn next<R: TryRead>(state: &mut ReadState, socket: &mut R, token: Token)
        -> Option<super::Packet<Vec<u8>>>
    {
        loop {
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
                                    buf: Take::new(Vec::with_capacity(len as usize), len as usize),
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
                NextReadState::Same => return None,
                NextReadState::Next(next) => *state = next,
                NextReadState::Reset(packet) => {
                    *state = ReadState::init();
                    return Some(packet)
                }
            }
        }
    }
}
