// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use byteorder::{BigEndian, WriteBytesExt};
use bytes::Buf;
use futures::Async;
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{self, Cursor};
use std::mem;
use tokio_proto::TryWrite;

/// The means of communication between client and server.
#[derive(Clone, Debug)]
pub struct Packet {
    /// (payload_len: u64, payload)
    ///
    /// The payload is typically a serialized message.
    pub buf: Cursor<Vec<u8>>,
}

impl Packet {
    /// Creates a new packet, (len, payload)
    pub fn serialize<S>(message: &S) -> Result<Packet, bincode::SerializeError>
        where S: Serialize
    {
        let payload_len = bincode::serialized_size(message);

        // (len, message)
        let mut buf = Vec::with_capacity(mem::size_of::<u64>() + payload_len as usize);

        buf.write_u64::<BigEndian>(payload_len).unwrap();
        bincode::serialize_into(&mut buf, message, SizeLimit::Infinite)?;
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
    fn try_write<W: TryWrite>(&mut self, stream: &mut W) -> io::Result<NextWriteAction> {
        while let Some(bytes_written) = stream.try_write_buf(self)? {
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
    pub fn next<W: TryWrite>(state: &mut Option<Packet>,
                                        socket: &mut W,
                                        outbound: &mut VecDeque<Packet>)
                                        -> io::Result<Async<()>> {
        loop {
            let update = match *state {
                None => {
                    match outbound.pop_front() {
                        Some(packet) => {
                            let size = packet.buf.remaining() as u64;
                            debug_assert!(size >= mem::size_of::<u64>() as u64);
                            NextWriteState::Next(packet)
                        }
                        None => return Ok(Async::Ready(())),
                    }
                }
                Some(ref mut packet) => {
                    match BufExt::try_write(&mut packet.buf, socket)? {
                        NextWriteAction::Stop => NextWriteState::Nothing,
                        NextWriteAction::Continue => return Ok(Async::NotReady),
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
