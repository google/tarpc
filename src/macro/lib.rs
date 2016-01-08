#![allow(dead_code)]
extern crate rustc_serialize;
                
macro_rules! rpc {
    ($server:ident: $($fn_name:ident($in_:ty) -> $out:ty;)* ) => {
        mod $server {
            use rustc_serialize::json;
            use std::net::{TcpListener, TcpStream};
            use std::thread;
            use std::io::{self, Read, Write};

            pub trait Service: Clone + Send {
                $(
                    fn $fn_name(&self, $in_) -> $out;
                )*

                fn handle_request(self, mut conn: TcpStream) -> Result<(), io::Error> {
                    let mut s = String::new();
                    try!(conn.read_to_string(&mut s));
                    let request: Request = json::decode(&s).unwrap();
                    let response = match request {
                        $(
                            Request::$fn_name(in_) => {
                                Response::$fn_name(self.$fn_name(in_))
                            }
                        )*
                    };
                    conn.write_all(json::encode(&response).unwrap().as_bytes())
                }
            }
            
            #[allow(non_camel_case_types)]
            #[derive(RustcEncodable, RustcDecodable)]
            enum Request {
                $(
                    $fn_name($in_),
                )*
            }
            
            #[allow(non_camel_case_types)]
            #[derive(RustcEncodable, RustcDecodable)]
            enum Response {
                $(
                    $fn_name($out),
                )*
            }
            
            pub struct Server<S: 'static + Service>(S);
            
            impl<S: Service> Server<S> {
                pub fn new(service: S) -> Server<S> {
                    Server(service)
                }

                pub fn serve(&self, listener: TcpListener) -> io::Error {
                    for conn in listener.incoming() {
                        let conn = match conn {
                            Err(err) => return err,
                            Ok(c) => c,
                        };
                        let service = self.0.clone();
                        thread::spawn(move || {
                            if let Err(err) = service.handle_request(conn) {
                                println!("error handling connection: {:?}", err);
                            }
                        });
                    }
                    unreachable!()
                }
            }
        }
    }
}

rpc!(my_server:
    hello(String) -> ();
    add((i32, i32)) -> i32;
);

use my_server::*;

impl Service for () {
    fn hello(&self, s: String) {
        println!("Hello, {}", s);
    }
    
    fn add(&self, (x, y): (i32, i32)) -> i32 {
        x + y
    }
}

fn main() {
    let server = Server::new(());
}
