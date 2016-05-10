use byteorder::{BigEndian, ByteOrder};
use mio::{EventSet, Token, TryWrite};
use mio::tcp::TcpStream;
use self::WriteState::*;
use std::collections::VecDeque;
use std::mem;
use std::io;
use super::{Data, Packet};

enum NextWriteAction {
    Stop,
    Continue,
}

pub struct Writer<D> {
    written: usize,
    data: D,
}

impl<D> Writer<D> {
    /// Writes data to stream. Returns Ok(true) if all data has been written or Ok(false) if
    /// there's still data to write.
    fn try_write(&mut self, stream: &mut TcpStream) -> io::Result<NextWriteAction>
        where D: Data
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

impl VecWriter {
    fn from_vec(data: Vec<u8>) -> Self {
        Writer {
            written: 0,
            data: data,
        }
    }
}

/// A state machine that writes packets in non-blocking fashion.
pub enum WriteState {
    WriteId {
        id: U64Writer,
        size: U64Writer,
        payload: Option<VecWriter>,
    },
    WriteSize {
        size: U64Writer,
        payload: Option<VecWriter>,
    },
    WriteData(VecWriter),
}

enum NextWriteState {
    Same,
    Nothing,
    Next(WriteState),
}

impl WriteState {
    pub fn next(state: &mut Option<WriteState>,
                socket: &mut TcpStream,
                outbound: &mut VecDeque<Packet>,
                interest: &mut EventSet,
                token: Token) {
        let update = match *state {
            None => {
                match outbound.pop_front() {
                    Some(packet) => {
                        let size = packet.payload.len() as u64;
                        debug!("WriteState {:?}: Packet: id: {}, size: {}",
                               token,
                               packet.id,
                               size);
                        NextWriteState::Next(WriteState::WriteId {
                            id: U64Writer::from_u64(packet.id),
                            size: U64Writer::from_u64(size),
                            payload: if packet.payload.is_empty() {
                                None
                            } else {
                                Some(VecWriter::from_vec(packet.payload))
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
                interest.insert(EventSet::readable());
                debug!("Remaining interests: {:?}", interest);
            }
            NextWriteState::Same => {}
        }
    }
}
