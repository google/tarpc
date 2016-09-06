// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {futures, serde};
use bincode::{SizeLimit, serde as bincode};
use std::{io, thread};
use std::collections::VecDeque;
use std::sync::mpsc;
use tokio_core::{Loop, LoopHandle};
use tokio_proto::io::{Readiness, Transport};
use tokio_proto::pipeline::Frame;

lazy_static! {
    #[doc(hidden)]
    pub static ref LOOP_HANDLE: LoopHandle = {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut lupe = Loop::new().unwrap();
            tx.send(lupe.handle()).unwrap();
            // Run forever
            lupe.run(futures::empty::<(), !>()).unwrap();
        });
        rx.recv().unwrap()
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

/// Deserialize a buffer into a `D` and its ID. On error, returns `tarpc::Error`.
pub fn deserialize<D: serde::Deserialize>(mut buf: &[u8]) -> Result<D, bincode::DeserializeError> {
    bincode::deserialize_from(&mut buf, SizeLimit::Infinite)
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
    where T: io::Read + io::Write + Readiness,
{
    type In = Frame<Packet, io::Error>;
    type Out = Frame<Vec<u8>, io::Error>;

    fn read(&mut self) -> io::Result<Option<Frame<Vec<u8>, io::Error>>> {
        self.read_state.next(&mut self.stream)
    }

    fn write(&mut self, req: Frame<Packet, io::Error>) -> io::Result<Option<()>> {
        self.outbound.push_back(req.unwrap_msg());
        self.flush()
    }

    fn flush(&mut self) -> io::Result<Option<()>> {
        writer::NextWriteState::next(&mut self.head, &mut self.stream, &mut self.outbound)
    }
}
