// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use serde;
use std::collections::VecDeque;
use std::io::{self, Cursor};
use std::sync::Mutex;
use tokio::io::{Readiness, Transport};
use tokio::proto::pipeline::Frame;
use tokio::reactor::{Reactor, ReactorHandle};

lazy_static! {
    pub static ref REACTOR: Mutex<ReactorHandle> = {
        let reactor = Reactor::default().unwrap();
        let handle = reactor.handle();
        reactor.spawn();
        Mutex::new(handle)
    };
}


pub use self::writer::Packet;

pub mod reader;
pub mod writer;

/// A helper trait to provide the `map_non_block` function on Results.
trait MapNonBlock<T> {
    /// Maps a `Result<T>` to a `Result<Option<T>>` by converting
    /// operation-would-block errors into `Ok(None)`.
    fn map_non_block(self) -> io::Result<Option<T>>;
}

impl<T> MapNonBlock<T> for io::Result<T> {
    fn map_non_block(self) -> io::Result<Option<T>> {
        use std::io::ErrorKind::WouldBlock;

        match self {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                if let WouldBlock = err.kind() {
                    Ok(None)
                } else {
                    Err(err)
                }
            }
        }
    }
}

/// Serialize `s`, left-padding with 8 bytes.
///
/// Returns `Vec<u8>` if successful, otherwise `tarpc::Error`.
pub fn serialize<S: serde::Serialize>(s: &S) -> ::Result<Vec<u8>> {
    let mut buf = vec![0; 8];
    try!(bincode::serialize_into(&mut buf, s, SizeLimit::Infinite));
    Ok(buf)
}

/// Deserialize a buffer into a `D` and its ID. On error, returns `tarpc::Error`.
pub fn deserialize<D: serde::Deserialize>(buf: &[u8]) -> ::Result<D> {
    bincode::deserialize_from(&mut Cursor::new(&buf), SizeLimit::Infinite).map_err(Into::into)
}

pub struct TarpcTransport<T> {
    stream: T,
    read_state: reader::ReadState,
    outbound: VecDeque<Packet>,
    head: Option<Packet>,
}

impl<T> TarpcTransport<T> {
    pub fn new(stream: T) -> Self {
        TarpcTransport {
            stream: stream,
            read_state: reader::ReadState::init(),
            outbound: VecDeque::new(),
            head: None,
        }
    }
}

impl<T> Readiness for TarpcTransport<T>
    where T: Readiness
{
    fn is_readable(&self) -> bool {
        self.stream.is_readable()
    }

    fn is_writable(&self) -> bool {
        // Always allow writing... this isn't really the best strategy to do in
        // practice, but it is the easiest to implement in this case. The
        // number of in-flight requests can be controlled using the pipeline
        // dispatcher.
        true
    }
}

impl<T> Transport for TarpcTransport<T>
    where T: io::Read + io::Write + Readiness
{
    type In = Frame<Packet, ::Error>;
    type Out = Frame<Vec<u8>, ::Error>;

    fn read(&mut self) -> io::Result<Option<Frame<Vec<u8>, ::Error>>> {
        self.read_state.next(&mut self.stream)
    }

    fn write(&mut self, req: Frame<Packet, ::Error>) -> io::Result<Option<()>> {
        match req {
            Frame::Message(msg) => {
                self.outbound.push_back(msg);
                self.flush()
            }
            Frame::Error(_) => unimplemented!(),
            Frame::Done => unimplemented!(),
        }
    }

    fn flush(&mut self) -> io::Result<Option<()>> {
        writer::NextWriteState::next(&mut self.head, &mut self.stream, &mut self.outbound)
    }
}

#[test]
fn vec_serialization() {
    use std::mem;

    let v = vec![1, 2, 3, 4, 5];
    let serialized = serialize(&v).unwrap();
    assert_eq!(v,
               deserialize::<Vec<u8>>(&serialized[mem::size_of::<u64>()..]).unwrap());
}
