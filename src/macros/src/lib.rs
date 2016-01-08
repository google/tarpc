#![feature(concat_idents)]
extern crate rustc_serialize;
extern crate byteorder;
                
#[macro_export]
macro_rules! rpc_service {
    ($server:ident: $($fn_name:ident($in_:ty) -> $out:ty;)* ) => {
        mod $server {
            use rustc_serialize::json;
            use std::net::{TcpListener, TcpStream};
            use std::thread;
            use std::io::{self, Read, Write};
            use byteorder::{ReadBytesExt, WriteBytesExt, BigEndian};

            pub trait Service: Clone + Send {
                $(
                    fn $fn_name(&self, $in_) -> $out;
                )*

                fn handle_request(self, mut conn: TcpStream) -> Result<(), io::Error> {
                    loop {
                        let len = try!(conn.read_u64::<BigEndian>());
                        let mut buf = vec![0; len as usize];
                        try!(conn.read_exact(&mut buf));
                        let s = String::from_utf8(buf).unwrap();
                        let request: Request = json::decode(&s).unwrap();
                        match request {
                            $(
                                Request::$fn_name(in_) => {
                                    let resp = self.$fn_name(in_);
                                    let resp = json::encode(&resp).unwrap();
                                    try!(conn.write_u64::<BigEndian>(resp.len() as u64));
                                    try!(conn.write_all(resp.as_bytes()));
                                }
                            )*
                        }
                    }
                }
            }
            
            #[allow(non_camel_case_types)]
            #[derive(Debug, RustcEncodable, RustcDecodable)]
            enum Request {
                $(
                    $fn_name($in_),
                )*
            }

            pub struct Client(pub TcpStream);

            impl Client {
                $(
                    pub fn $fn_name(&mut self, in_: $in_) -> Result<$out, io::Error> {
                        let ref mut conn = self.0;
                        let request = Request::$fn_name(in_);
                        let request = json::encode(&request).unwrap();
                        try!(conn.write_u64::<BigEndian>(request.len() as u64));
                        try!(conn.write_all(request.as_bytes()));
                        let len = try!(conn.read_u64::<BigEndian>());
                        let mut buf = vec![0; len as usize];
                        try!(conn.read_exact(&mut buf));
                        let s = String::from_utf8(buf).unwrap();
                        Ok(json::decode(&s).unwrap())
                    }
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

#[cfg(test)]
mod test {
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use self::my_server::*;

    rpc_service!(my_server:
        hello(super::Foo) -> super::Foo;
        add((i32, i32)) -> i32;
    );

    #[derive(PartialEq, Debug, RustcEncodable, RustcDecodable)]
    pub struct Foo {
        message: String
    }

    impl Service for () {
        fn hello(&self, s: Foo) -> Foo {
            Foo{message: format!("Hello, {}", &s.message)}
        }

        fn add(&self, (x, y): (i32, i32)) -> i32 {
            x + y
        }
    }

    #[test]
    fn simple_test() {
        println!("Starting");
        let listener = TcpListener::bind("127.0.0.1:9000").unwrap();
        thread::spawn(|| {
            let server = Server::new(());
            println!("Server running");
            server.serve(listener);
        });
        let mut client = Client(TcpStream::connect("127.0.0.1:9000").unwrap());
        assert_eq!(3, client.add((1, 2)).unwrap());
        let foo = Foo{message: "Adam".into()};
        let want = Foo{message: format!("Hello, {}", &foo.message)};
        assert_eq!(want, client.hello(Foo{message: "Adam".into()}).unwrap());
    }
}
