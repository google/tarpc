use mio::{Token, TryRead};
use mio::tcp::TcpStream;
use self::ReadState::*;
use std::io;
use super::{Data, Packet};

pub struct Reader<D> {
    read: usize,
    data: D,
}

enum NextReadAction<D>
    where D: Data
{
    Continue,
    Stop(D::Read),
}

impl<D> Reader<D> {
    fn try_read(&mut self, stream: &mut TcpStream) -> io::Result<NextReadAction<D>>
        where D: Data
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

enum NextReadState {
    Same,
    Next(ReadState),
    Reset(Packet),
}

impl ReadState {
    pub fn init() -> ReadState {
        ReadId(U64Reader::new())
    }

    pub fn next(state: &mut ReadState, socket: &mut TcpStream, token: Token) -> Option<Packet> {
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
                                id: id,
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
                            id: id,
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
