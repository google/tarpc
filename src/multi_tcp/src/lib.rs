use std::fmt;
use std::net::TcpStream;
use std::thread;
use std::sync::mpsc::{
    channel,
    sync_channel,
    Sender,
    SyncSender,
    Receiver,
};

fn read<T, F, E>(mut stream: TcpStream, decode: F, tx: SyncSender<T>)
    where F: Send + 'static + Fn(&mut TcpStream) -> Result<T, E>,
          T: Send + 'static,
          E: fmt::Debug + Send + 'static
{
    loop {
        let t = decode(&mut stream).unwrap();
        if let Err(_) = tx.send(t) {
            break;
        }
    }
}

struct SendHelper<T, E> {
    value: T,
    result: Sender<Result<(), E>>,
}

fn write<T, F, E>(mut stream: TcpStream, encode: F) -> Sender<SendHelper<T, E>>
    where F: Send + 'static + Fn(&mut TcpStream, &T) -> Result<(), E>,
          T: Send + 'static,
          E: Send + 'static
{
    let (tx, rx) = channel();
    thread::spawn(move || {
        loop {
            let helper: SendHelper<T, E> = match rx.recv() {
                Ok(h) => h,
                Err(_) => {
                    break;
                }
            };
            helper.result.send(encode(&mut stream, &helper.value)).unwrap();
        }
    });
    tx
}

pub struct MultiStream<Request, E> {
    tx: Sender<SendHelper<Request, E>>,
}

impl<Request, E> MultiStream<Request, E>
    where Request: Send + 'static,
          E: fmt::Debug + Send + 'static
{
    pub fn new<Reply, F, G>(
        stream: TcpStream,
        encode: F,
        decode: G) -> (Self, Receiver<Reply>)
        where Reply: Send + 'static,
              F: Send + 'static + Fn(&mut TcpStream, &Request) -> Result<(), E>,
              G: Send + 'static + Fn(&mut TcpStream) -> Result<Reply, E>
    {
        let read_stream = stream.try_clone().unwrap();
        let ms = MultiStream{tx: write(stream, encode)};
        let (reply_tx, reply_rx) = sync_channel(0);
        thread::spawn(move || read(read_stream, decode, reply_tx));
        (ms, reply_rx)
    }

    pub fn with_sync_sender<Reply, F, G>(
        stream: TcpStream,
        encode: F,
        decode: G,
        reply_tx: SyncSender<Reply>) -> Self
        where Reply: Send + 'static,
              F: Send + 'static + Fn(&mut TcpStream, &Request) -> Result<(), E>,
              G: Send + 'static + Fn(&mut TcpStream) -> Result<Reply, E>
    {
        let read_stream = stream.try_clone().unwrap();
        thread::spawn(move || read(read_stream, decode, reply_tx));
        MultiStream{tx: write(stream, encode)}
    }


    pub fn write(&self, value: Request) -> Result<(), E> {
        let my_tx = self.tx.clone();
        let (reply_tx, reply_rx) = channel();
        let helper = SendHelper{
            value: value,
            result: reply_tx,
        };
        my_tx.send(helper).unwrap();
        reply_rx.recv().unwrap()
    }
}

#[cfg(test)]
mod test {
    use super::MultiStream;
    use std::net::{TcpStream, TcpListener};
    use std::sync::mpsc::Receiver;
    use std::io::{Write, Read};

    fn pair() -> (TcpStream, Receiver<TcpStream>) {
        let addr = "127.0.0.1:9000";
        let recv_stream = listen(TcpListener::bind(addr).unwrap());
        (TcpStream::connect(addr).unwrap(), recv_stream)
    }

    fn write_byte(stream: &mut TcpStream, v: u8) -> Result<(), ()> {
        stream.write(&[v]).unwrap();
        Ok(())
    }

    fn read_byte(stream: &mut TcpStream) -> Result<u8, ()> {
        let mut buf = [0u8];
        stream.read_exact(&mut buf[..]).unwrap();
        Ok(buf[0])
    }

    #[test]
    fn test_thing() {
        let (stream, listener) = pair();
        let (ms, reader) : (MultiStream<u8, u8, ()>, Receiver<u8>) =
            MultiStream::new(stream, |s, v| write_byte(s, *v), |s| read_byte(s));
        ms.write(5).expect("writing 5");
        let mut srv_stream = listener.accept().unwrap().0;
        assert_eq!(5, read_byte(&mut srv_stream).expect("read 5"));
        write_byte(&mut srv_stream, 10).expect("write 10");
        assert_eq!(10, reader.recv().expect("reading 10"));
    }
}
