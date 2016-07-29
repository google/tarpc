// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use byteorder::{BigEndian, ReadBytesExt};
use bytes::{MutBuf, Take};
use mio::Token;
use self::ReadState::*;
use std::io::{self, Read};
use std::mem;
use super::{MapNonBlock, RpcId};

pub trait TryRead {
    fn try_read_buf<B: MutBuf>(&mut self, buf: &mut B) -> io::Result<Option<usize>>
        where Self: Sized
    {
        // Reads the length of the slice supplied by buf.mut_bytes into the buffer
        // This is not guaranteed to consume an entire datagram or segment.
        // If your protocol is msg based (instead of continuous stream) you should
        // ensure that your buffer is large enough to hold an entire segment (1532 bytes if not jumbo
        // frames)
        let res = self.try_read(unsafe { buf.mut_bytes() });

        if let Ok(Some(cnt)) = res {
            unsafe {
                buf.advance(cnt);
            }
        }

        res
    }

    fn try_read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>>;
}

impl<T: Read> TryRead for T {
    fn try_read(&mut self, dst: &mut [u8]) -> io::Result<Option<usize>> {
        self.read(dst).map_non_block()
    }
}

#[derive(Debug)]
pub struct Packet {
    pub buf: Vec<u8>,
}

impl Packet {
    pub fn id(&self) -> RpcId {
        RpcId((&self.buf[..8]).read_u64::<BigEndian>().unwrap())
    }
}

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
                debug!("Reader: would block.");
                return Ok(NextReadAction::Continue);
            }

            if !self.has_remaining() {
                trace!("Reader: finished.");
                return Ok(NextReadAction::Stop(self.take()));
            }
        }
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
    /// Tracks how many bytes of the message size have been read.
    ReadLen(U64Reader),
    /// Tracks read progress.
    ReadData(Take<Vec<u8>>),
}

#[derive(Debug)]
enum NextReadState {
    Same,
    Next(ReadState),
    Reset(Packet),
}

impl ReadState {
    pub fn init() -> ReadState {
        ReadLen(U64Reader::new())
    }

    pub fn next<R: TryRead>(state: &mut ReadState, socket: &mut R, token: Token) -> Option<Packet> {
        loop {
            let next = match *state {
                ReadLen(ref mut len) => {
                    match len.try_read(socket) {
                        Ok(NextReadAction::Continue) => NextReadState::Same,
                        Ok(NextReadAction::Stop(len)) => {
                            debug!("ReadLen {:?}: transitioning to reading payload({})",
                                   token,
                                   len);
                            let buf = Vec::with_capacity(len as usize);
                            NextReadState::Next(ReadData(Take::new(buf, len as usize)))
                        }
                        Err(e) => {
                            debug!("ReadState {:?}: read err, {:?}", token, e);
                            NextReadState::Same
                        }
                    }
                }
                ReadData(ref mut buf) => {
                    match buf.try_read(socket) {
                        Ok(NextReadAction::Continue) => NextReadState::Same,
                        Ok(NextReadAction::Stop(buf)) => NextReadState::Reset(Packet { buf: buf }),
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
                    return Some(packet);
                }
            }
        }
    }
}
