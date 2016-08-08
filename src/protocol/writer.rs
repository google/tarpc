// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use byteorder::{BigEndian, WriteBytesExt};
use bytes::Buf;
use serde::Serialize;
use std::collections::VecDeque;
use std::mem;
use std::io::{self, Cursor};

mod try_write {
    use bytes::Buf;
    use protocol::MapNonBlock;
    use std::io::{self, Write};

    pub trait TryWrite {
        fn try_write_buf<B: Buf>(&mut self, buf: &mut B) -> io::Result<Option<usize>>
            where Self: Sized
        {
            let res = self.try_write(buf.bytes());

            if let Ok(Some(cnt)) = res {
                buf.advance(cnt);
            }

            res
        }

        fn try_write(&mut self, buf: &[u8]) -> io::Result<Option<usize>>;
    }

    impl<T: Write> TryWrite for T {
        fn try_write(&mut self, src: &[u8]) -> io::Result<Option<usize>> {
            self.write(src).map_non_block()
        }
    }
}

/// The means of communication between client and server.
#[derive(Clone, Debug)]
pub struct Packet {
    /// (id: u64, payload_len: u64, payload)
    ///
    /// The payload is typically a serialized message.
    pub buf: Cursor<Vec<u8>>,
}

impl Packet {
    /// Creates a new packet, (len, payload)
    pub fn new<S>(request: &S) -> ::Result<Packet>
        where S: Serialize
    {
        let payload_len = bincode::serialized_size(request);

        // (len, request)
        let mut buf = Vec::with_capacity(mem::size_of::<u64>() + payload_len as usize);

        buf.write_u64::<BigEndian>(payload_len).unwrap();
        try!(bincode::serialize_into(&mut buf, request, SizeLimit::Infinite));
        Ok(Packet { buf: Cursor::new(buf) })
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
    fn try_write<W: try_write::TryWrite>(&mut self, stream: &mut W) -> io::Result<NextWriteAction> {
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

#[derive(Debug)]
pub enum NextWriteState {
    Nothing,
    Next(Packet),
}

impl NextWriteState {
    pub fn next<W: try_write::TryWrite>(state: &mut Option<Packet>,
                                        socket: &mut W,
                                        outbound: &mut VecDeque<Packet>)
                                        -> io::Result<Option<()>> {
        loop {
            let update = match *state {
                None => {
                    match outbound.pop_front() {
                        Some(packet) => {
                            let size = packet.buf.remaining() as u64;
                            debug_assert!(size >= mem::size_of::<u64>() as u64);
                            NextWriteState::Next(packet)
                        }
                        None => return Ok(Some(())),
                    }
                }
                Some(ref mut packet) => {
                    match try!(packet.buf.try_write(socket)) {
                        NextWriteAction::Stop => NextWriteState::Nothing,
                        NextWriteAction::Continue => return Ok(None),
                    }
                }
            };
            match update {
                NextWriteState::Next(next) => *state = Some(next),
                NextWriteState::Nothing => {
                    *state = None;
                }
            }
        }
    }
}
