#[macro_use]
extern crate macros;
extern crate rustc_serialize;
extern crate byteorder;

use std::net::{TcpListener, TcpStream};

rpc!(my_server:
    hello(String) -> String;
    add((i32, i32)) -> i32;
);

use my_server::*;

impl Service for () {
    fn hello(&self, s: String) -> String {
        format!("Hello, {}", s)
    }
    
    fn add(&self, (x, y): (i32, i32)) -> i32 {
        x + y
    }
}

fn main() {
    println!("Starting");
    let listener = TcpListener::bind("127.0.0.1:9000").unwrap();
    std::thread::spawn(|| {
        let server = Server::new(());
        println!("Server running");
        server.serve(listener);
    });
    let mut client = Client(TcpStream::connect("127.0.0.1:9000").unwrap());
    println!("Client running");
    println!("add((1, 2)) => {}", client.add((1, 2)).unwrap());
    println!("hello(\"adam\") => {:?}", client.hello("Adam".into()));
}
