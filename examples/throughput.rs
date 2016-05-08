#[macro_use]
extern crate tarpc;
extern crate env_logger;

use std::time;
use std::net;
use std::thread;
use std::io::{Read, Write};

fn gen_vec(size: usize) -> Vec<u8> {
    let mut vec: Vec<u8> = Vec::with_capacity(size);
    for i in 0..size {
        vec.push((i % 1 << 8) as u8);
    }
    vec
}

service! {
    rpc read(size: u32) -> Vec<u8>;
}

struct Server;

impl Service for Server {
    fn read(&mut self, mut ctx: Ctx, size: u32) {
        ctx.read(&gen_vec(size as usize));
    }
}

const CHUNK_SIZE: u32 = 1 << 18;

fn bench_tarpc(target: u64) {
    let handle = Server.spawn("0.0.0.0:0").unwrap();
    let client = BlockingClient::spawn(handle.local_addr).unwrap();
    let start = time::Instant::now();
    let mut nread = 0;
    while nread < target {
        client.read(&CHUNK_SIZE).unwrap();
        nread += CHUNK_SIZE as u64;
    }
    let duration = time::Instant::now() - start;
    println!("TARPC: {}MB/s",
             (target as f64 / (1024f64 * 1024f64)) /
             (duration.as_secs() as f64 + duration.subsec_nanos() as f64 / 10E9));
}

fn bench_tcp(target: u64) {
    let l = net::TcpListener::bind("0.0.0.0:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = l.accept().unwrap();
        let mut vec = gen_vec(CHUNK_SIZE as usize);
        while let Ok(_) = stream.write_all(&vec[..]) {
            vec = gen_vec(CHUNK_SIZE as usize);
        }
    });
    let mut stream = net::TcpStream::connect(&addr).unwrap();
    let mut buf = vec![0; CHUNK_SIZE as usize];
    let start = time::Instant::now();
    let mut nread = 0;
    while nread < target {
        stream.read_exact(&mut buf[..]).unwrap();
        nread += CHUNK_SIZE as u64;
    }
    let duration = time::Instant::now() - start;
    println!("TCP:   {}MB/s",
             (target as f64 / (1024f64 * 1024f64)) /
             (duration.as_secs() as f64 + duration.subsec_nanos() as f64 / 10E9));
}

fn main() {
    let _ = env_logger::init();
    bench_tarpc(256 << 20);
    bench_tcp(256 << 20);
}
