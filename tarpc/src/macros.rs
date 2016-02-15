// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[doc(hidden)]
#[macro_export]
macro_rules! as_item { ($i:item) => {$i} }

// Required because if-let can't be used with irrefutable patterns, so it needs
// to be special cased.
#[doc(hidden)]
#[macro_export]
macro_rules! client_methods {
    (
        { $(#[$attr:meta])* }
        $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty
    ) => (
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: $in_),*) -> $crate::Result<$out> {
            let reply = try!((self.0).rpc(request_variant!($fn_name $($arg),*)));
            let __Reply::$fn_name(reply) = reply;
            Ok(reply)
        }
    );
    ($(
            { $(#[$attr:meta])* }
            $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty
    )*) => ( $(
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: $in_),*) -> $crate::Result<$out> {
            let reply = try!((self.0).rpc(request_variant!($fn_name $($arg),*)));
            if let __Reply::$fn_name(reply) = reply {
                Ok(reply)
            } else {
                panic!("Incorrect reply variant returned from protocol::Clientrpc; expected `{}`, but got {:?}", stringify!($fn_name), reply);
            }
        }
    )*);
}

// Required because if-let can't be used with irrefutable patterns, so it needs
// to be special cased.
#[doc(hidden)]
#[macro_export]
macro_rules! async_client_methods {
    (
        { $(#[$attr:meta])* }
        $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty
    ) => (
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: $in_),*) -> Future<$out> {
            fn mapper(reply: __Reply) -> $out {
                let __Reply::$fn_name(reply) = reply;
                reply
            }
            let reply = (self.0).rpc_async(request_variant!($fn_name $($arg),*));
            Future {
                future: reply,
                mapper: mapper,
            }
        }
    );
    ($(
            { $(#[$attr:meta])* }
            $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty
    )*) => ( $(
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: $in_),*) -> Future<$out> {
            fn mapper(reply: __Reply) -> $out {
                if let __Reply::$fn_name(reply) = reply {
                    reply
                } else {
                    panic!("Incorrect reply variant returned from protocol::Clientrpc; expected `{}`, but got {:?}", stringify!($fn_name), reply);
                }
            }
            let reply = (self.0).rpc_async(request_variant!($fn_name $($arg),*));
            Future {
                future: reply,
                mapper: mapper,
            }
        }
    )*);
}

// Required because enum variants with no fields can't be suffixed by parens
#[doc(hidden)]
#[macro_export]
macro_rules! define_request {
    ($(@($($finished:tt)*))* --) => (as_item!(
            #[allow(non_camel_case_types)]
            #[derive(Debug, Serialize, Deserialize)]
            enum __Request { $($($finished)*),* }
    ););
    ($(@$finished:tt)* -- $name:ident() $($req:tt)*) =>
        (define_request!($(@$finished)* @($name) -- $($req)*););
    ($(@$finished:tt)* -- $name:ident $args: tt $($req:tt)*) =>
        (define_request!($(@$finished)* @($name $args) -- $($req)*););
    ($($started:tt)*) => (define_request!(-- $($started)*););
}

// Required because enum variants with no fields can't be suffixed by parens
#[doc(hidden)]
#[macro_export]
macro_rules! request_variant {
    ($x:ident) => (__Request::$x);
    ($x:ident $($y:ident),+) => (__Request::$x($($y),+));
}

/// The main macro that creates RPC services.
///
/// Rpc methods are specified, mirroring trait syntax:
///
/// ```
/// # #![feature(custom_derive, plugin)]
/// # #![plugin(serde_macros)]
/// # #[macro_use] extern crate tarpc;
/// # extern crate serde;
/// # fn main() {}
/// # service! {
/// #[doc="Say hello"]
/// rpc hello(name: String) -> String;
/// # }
/// ```
///
/// Attributes can be attached to each rpc. These attributes
/// will then be attached to the generated `Service` trait's
/// corresponding method, as well as to the `Client` stub's rpcs methods.
///
/// The following items are expanded in the enclosing module:
///
/// * `Service` -- the trait defining the RPC service
/// * `Client` -- a client that makes synchronous requests to the RPC server
/// * `AsyncClient` -- a client that makes asynchronous requests to the RPC server
/// * `Future` -- a handle for asynchronously retrieving the result of an RPC
/// * `serve` -- the function that starts the RPC server
///
/// **Warning**: In addition to the above items, there are a few expanded items that
/// are considered implementation details. As with the above items, shadowing
/// these item names in the enclosing module is likely to break things in confusing
/// ways:
///
/// * `__Server` -- an implementation detail
/// * `__Request` -- an implementation detail
/// * `__Reply` -- an implementation detail
#[macro_export]
macro_rules! service {
    (
        $( $tokens:tt )*
    ) => {
        service_inner! {{
            $( $tokens )*
        }}
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! service_inner {
    // Pattern for when the next rpc has an implicit unit return type
    (
        {
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* );

            $( $unexpanded:tt )*
        }
        $( $expanded:tt )*
    ) => {
        service_inner! {
            { $( $unexpanded )* }

            $( $expanded )*

            $(#[$attr])*
            rpc $fn_name( $( $arg : $in_ ),* ) -> ();
        }
    };
    // Pattern for when the next rpc has an explicit return type
    (
        {
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty;

            $( $unexpanded:tt )*
        }
        $( $expanded:tt )*
    ) => {
        service_inner! {
            { $( $unexpanded )* }

            $( $expanded )*

            $(#[$attr])*
            rpc $fn_name( $( $arg : $in_ ),* ) -> $out;
        }
    };
    // Pattern when all return types have been expanded
    (
        { } // none left to expand
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty;
        )*
    ) => {
        #[doc="Defines the RPC service"]
        pub trait Service: Send + Sync {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $out;
            )*
        }

        impl<P, S> Service for P
            where P: Send + Sync + ::std::ops::Deref<Target=S>,
                  S: Service
        {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $out {
                    Service::$fn_name(&**self, $($arg),*)
                }
            )*
        }

        define_request!($($fn_name($($in_),*))*);

        #[allow(non_camel_case_types)]
        #[derive(Debug, Serialize, Deserialize)]
        enum __Reply {
            $(
                $fn_name($out),
            )*
        }

        /// An asynchronous RPC call
        pub struct Future<T> {
            future: $crate::protocol::Future<__Reply>,
            mapper: fn(__Reply) -> T,
        }

        impl<T> Future<T> {
            /// Block until the result of the RPC call is available
            pub fn get(self) -> $crate::Result<T> {
                self.future.get().map(self.mapper)
            }
        }

        #[doc="The client stub that makes RPC calls to the server."]
        pub struct Client($crate::protocol::Client<__Request, __Reply>);

        impl Client {
            #[doc="Create a new client that connects to the given address."]
            pub fn new<A>(addr: A, timeout: ::std::option::Option<::std::time::Duration>)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs,
            {
                let inner = try!($crate::protocol::Client::new(addr, timeout));
                Ok(Client(inner))
            }

            client_methods!(
                $(
                    { $(#[$attr])* }
                    $fn_name($($arg: $in_),*) -> $out
                )*
            );

            #[doc="Attempt to clone the client object. This might fail if the underlying TcpStream clone fails"]
            pub fn try_clone(&self) -> ::std::io::Result<Self> {
                Ok(Client(try!(self.0.try_clone())))
            }
        }

        #[doc="The client stub that makes asynchronous RPC calls to the server."]
        pub struct AsyncClient($crate::protocol::Client<__Request, __Reply>);

        impl AsyncClient {
            #[doc="Create a new asynchronous client that connects to the given address."]
            pub fn new<A>(addr: A, timeout: ::std::option::Option<::std::time::Duration>)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs,
            {
                let inner = try!($crate::protocol::Client::new(addr, timeout));
                Ok(AsyncClient(inner))
            }

            async_client_methods!(
                $(
                    { $(#[$attr])* }
                    $fn_name($($arg: $in_),*) -> $out
                )*
            );

            #[doc="Attempt to clone the client object. This might fail if the underlying TcpStream clone fails"]
            pub fn try_clone(&self) -> ::std::io::Result<Self> {
                Ok(AsyncClient(try!(self.0.try_clone())))
            }
        }

        struct __Server<S: 'static + Service>(S);

        impl<S> $crate::protocol::Serve for __Server<S>
            where S: 'static + Service
        {
            type Request = __Request;
            type Reply = __Reply;
            fn serve(&self, request: __Request) -> __Reply {
                match request {
                    $(
                        request_variant!($fn_name $($arg),*) =>
                            __Reply::$fn_name((self.0).$fn_name($($arg),*)),
                     )*
                }
            }
        }

        #[doc="Start a running service."]
        pub fn serve<A, S>(addr: A,
                           service: S,
                           read_timeout: ::std::option::Option<::std::time::Duration>)
            -> $crate::Result<$crate::protocol::ServeHandle>
            where A: ::std::net::ToSocketAddrs,
                  S: 'static + Service
        {
            let server = ::std::sync::Arc::new(__Server(service));
            Ok(try!($crate::protocol::serve_async(addr, server, read_timeout)))
        }
    }
}

#[cfg(test)]
#[allow(dead_code)] // because we're testing that the macro expansion compiles
mod syntax_test {
    // Tests a service definition with a fn that takes no args
    mod qux {
        service! {
            rpc hello() -> String;
        }
    }

    // Tests a service definition with an attribute.
    mod bar {
        service! {
            #[doc="Hello bob"]
            rpc baz(s: String) -> String;
        }
    }

    // Tests a service with implicit return types.
    mod no_return {
        service! {
            rpc ack();
            rpc apply(foo: String) -> i32;
            rpc bi_consume(bar: String, baz: u64);
        }
    }
}

#[cfg(test)]
mod functional_test {
    extern crate env_logger;
    use std::time::Duration;

    fn test_timeout() -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    service! {
        rpc add(x: i32, y: i32) -> i32;
    }

    struct Server;

    impl Service for Server {
        fn add(&self, x: i32, y: i32) -> i32 {
            x + y
        }
    }

    #[test]
    fn simple() {
        let handle = serve( "localhost:0", Server, test_timeout()).unwrap();
        let client = Client::new(handle.local_addr(), None).unwrap();
        assert_eq!(3, client.add(1, 2).unwrap());
        drop(client);
        handle.shutdown();
    }

    #[test]
    fn simple_async() {
        let handle = serve("localhost:0", Server, test_timeout()).unwrap();
        let client = AsyncClient::new(handle.local_addr(), None).unwrap();
        assert_eq!(3, client.add(1, 2).get().unwrap());
        drop(client);
        handle.shutdown();
    }

    #[test]
    fn try_clone() {
        let handle = serve( "localhost:0", Server, test_timeout()).unwrap();
        let client1 = Client::new(handle.local_addr(), None).unwrap();
        let client2 = client1.try_clone().unwrap();
        assert_eq!(3, client1.add(1, 2).unwrap());
        assert_eq!(3, client2.add(1, 2).unwrap());
    }

    #[test]
    fn async_try_clone() {
        let handle = serve("localhost:0", Server, test_timeout()).unwrap();
        let client1 = AsyncClient::new(handle.local_addr(), None).unwrap();
        let client2 = client1.try_clone().unwrap();
        assert_eq!(3, client1.add(1, 2).get().unwrap());
        assert_eq!(3, client2.add(1, 2).get().unwrap());
    }

    // Tests that a server can be wrapped in an Arc; no need to run, just compile
    #[allow(dead_code)]
    fn serve_arc_server() {
        let _ = serve("localhost:0", ::std::sync::Arc::new(Server), None);
    }
}

#[cfg(test)]
#[allow(dead_code)] // generated Client isn't used in this benchmark
mod benchmark {
    extern crate env_logger;
    use ServeHandle;
    use std::sync::{Arc, Mutex};
    use test::Bencher;

    service! {
        rpc hello(s: String) -> String;
    }

    struct HelloServer;
    impl Service for HelloServer {
        fn hello(&self, s: String) -> String {
            format!("Hello, {}!", s)
        }
    }

    // Prevents resource exhaustion when benching
    lazy_static! {
        static ref HANDLE: Arc<Mutex<ServeHandle>> = {
            let handle = serve("localhost:0", HelloServer, None).unwrap();
            Arc::new(Mutex::new(handle))
        };
        static ref CLIENT: Arc<Mutex<AsyncClient>> = {
            let addr = HANDLE.lock().unwrap().local_addr().clone();
            let client = AsyncClient::new(addr, None).unwrap();
            Arc::new(Mutex::new(client))
        };
    }

    #[bench]
    fn hello(bencher: &mut Bencher) {
        let _ = env_logger::init();
        let client = CLIENT.lock().unwrap();
        let concurrency = 100;
        let mut futures = Vec::with_capacity(concurrency);
        let mut count = 0;
        bencher.iter(|| {
            futures.push(client.hello("Bob".into()));
            count += 1;
            if count % concurrency == 0 {
                // We can't block on each rpc call, otherwise we'd be
                // benchmarking latency instead of throughput. It's also
                // not ideal to call more than one rpc per iteration, because
                // it makes the output of the bencher harder to parse (you have
                // to mentally divide the number by `concurrency` to get
                // the ns / iter for one rpc
                for f in futures.drain(..) {
                    f.get().unwrap();
                }
            }
        });
    }
}
