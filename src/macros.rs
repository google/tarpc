// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

/// Serde re-exports required by macros. Not for general use.
pub mod serde {
    pub use serde::{Deserialize, Deserializer, Serialize, Serializer};
    /// Deserialization re-exports required by macros. Not for general use.
    pub mod de {
        pub use serde::de::{EnumVisitor, Error, VariantVisitor, Visitor};
    }
}

pub mod bincode {
    pub use bincode::serde::{deserialize_from, serialize};
    pub use bincode::SizeLimit;
}

/// Mio re-exports required by macros. Not for general use.
pub mod mio {
    pub use mio::tcp::TcpStream;
    pub use mio::{EventLoop, Token, Sender};
}

// Required because if-let can't be used with irrefutable patterns, so it needs
// to be special cased.
#[doc(hidden)]
#[macro_export]
macro_rules! client_methods {
    (
        { $(#[$attr:meta])* }
        $fn_name:ident( ($($arg:ident,)*) : ($($in_:ty,)*) ) -> $out:ty
    ) => (
        #[allow(unused)]
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: &$in_),*) -> $crate::Result<$out> {
            let reply = try!(try!((self.0).rpc(&__ClientSideRequest::$fn_name(&($($arg,)*)))).get());
            let __Reply::$fn_name(reply) = reply;
            ::std::result::Result::Ok(reply)
        }
    );
    ($(
            { $(#[$attr:meta])* }
            $fn_name:ident( ($( $arg:ident,)*) : ($($in_:ty, )*) ) -> $out:ty
    )*) => ( $(
        #[allow(unused)]
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: &$in_),*) -> $crate::Result<$out> {
            let reply = try!(try!((self.0).rpc(&__ClientSideRequest::$fn_name(&($($arg,)*)))).get());
            if let __Reply::$fn_name(reply) = reply {
                ::std::result::Result::Ok(reply)
            } else {
                panic!("Incorrect reply variant returned from rpc; expected `{}`, \
                       but got {:?}",
                       stringify!($fn_name),
                       reply);
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
        $fn_name:ident( ($( $arg:ident, )*) : ($( $in_:ty, )*) ) -> $out:ty
    ) => (
        #[allow(unused)]
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: &$in_),*) -> Future<$out> {
            fn mapper(reply: __Reply) -> $out {
                let __Reply::$fn_name(reply) = reply;
                reply
            }
            let reply = (self.0).rpc(&__ClientSideRequest::$fn_name(&($($arg,)*)));
            Future {
                future: reply,
                mapper: mapper,
            }
        }
    );
    ($(
            { $(#[$attr:meta])* }
            $fn_name:ident( ($( $arg:ident, )*) : ($( $in_:ty, )*) ) -> $out:ty
    )*) => ( $(
        #[allow(unused)]
        $(#[$attr])*
        pub fn $fn_name(&self, $($arg: &$in_),*) -> Future<$out> {
            fn mapper(reply: __Reply) -> $out {
                if let __Reply::$fn_name(reply) = reply {
                    reply
                } else {
                    panic!("Incorrect reply variant returned from rpc; expected `{}`, but got \
                           {:?}",
                           stringify!($fn_name),
                           reply);
                }
            }
            let reply = (self.0).rpc(&__ClientSideRequest::$fn_name(&($($arg,)*)));
            Future {
                future: reply,
                mapper: mapper,
            }
        }
    )*);
}

#[macro_export]
macro_rules! as_item {
    ($i:item) => {$i};
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_serialize {
    ($impler:ident, { $($lifetime:tt)* }, $(@($name:ident $n:expr))* -- #($_n:expr) ) => {
        as_item! {
            impl$($lifetime)* $crate::macros::serde::Serialize for $impler$($lifetime)* {
                #[inline]
                fn serialize<S>(&self, serializer: &mut S) -> ::std::result::Result<(), S::Error>
                    where S: $crate::macros::serde::Serializer
                {
                    match *self {
                        $(
                            $impler::$name(ref field) =>
                                $crate::macros::serde::Serializer::serialize_newtype_variant(
                                    serializer,
                                    stringify!($impler),
                                    $n,
                                    stringify!($name),
                                    field,
                                )
                        ),*
                    }
                }
            }
        }
    };
    // All args are wrapped in a tuple so we can use the newtype variant for each one.
    ($impler:ident, { $($lifetime:tt)* }, $(@$finished:tt)* -- #($n:expr) $name:ident($field:ty) $($req:tt)*) => (
        impl_serialize!($impler, { $($lifetime)* }, $(@$finished)* @($name $n) -- #($n + 1) $($req)*);
    );
    // Entry
    ($impler:ident, { $($lifetime:tt)* }, $($started:tt)*) => (impl_serialize!($impler, { $($lifetime)* }, -- #(0) $($started)*););
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_deserialize {
    ($impler:ident, $(@($name:ident $n:expr))* -- #($_n:expr) ) => (
        impl $crate::macros::serde::Deserialize for $impler {
            #[inline]
            fn deserialize<D>(deserializer: &mut D)
                -> ::std::result::Result<$impler, D::Error>
                where D: $crate::macros::serde::Deserializer
            {
                #[allow(non_camel_case_types, unused)]
                enum __Field {
                    $($name),*
                }
                impl $crate::macros::serde::Deserialize for __Field {
                    #[inline]
                    fn deserialize<D>(deserializer: &mut D)
                        -> ::std::result::Result<__Field, D::Error>
                        where D: $crate::macros::serde::Deserializer
                    {
                        struct __FieldVisitor;
                        impl $crate::macros::serde::de::Visitor for __FieldVisitor {
                            type Value = __Field;

                            #[inline]
                            fn visit_usize<E>(&mut self, value: usize)
                                -> ::std::result::Result<__Field, E>
                                where E: $crate::macros::serde::de::Error,
                            {
                                $(
                                    if value == $n {
                                        return ::std::result::Result::Ok(__Field::$name);
                                    }
                                )*
                                return ::std::result::Result::Err(
                                    $crate::macros::serde::de::Error::custom(
                                        format!("No variants have a value of {}!", value))
                                );
                            }
                        }
                        deserializer.deserialize_struct_field(__FieldVisitor)
                    }
                }

                struct __Visitor;
                impl $crate::macros::serde::de::EnumVisitor for __Visitor {
                    type Value = $impler;

                    #[inline]
                    fn visit<__V>(&mut self, mut visitor: __V)
                        -> ::std::result::Result<$impler, __V::Error>
                        where __V: $crate::macros::serde::de::VariantVisitor
                    {
                        match try!(visitor.visit_variant()) {
                            $(
                                __Field::$name => {
                                    let val = try!(visitor.visit_newtype());
                                    Ok($impler::$name(val))
                                }
                            ),*
                        }
                    }
                }
                const VARIANTS: &'static [&'static str] = &[
                    $(
                        stringify!($name)
                    ),*
                ];
                deserializer.deserialize_enum(stringify!($impler), VARIANTS, __Visitor)
            }
        }
    );
    // All args are wrapped in a tuple so we can use the newtype variant for each one.
    ($impler:ident, $(@$finished:tt)* -- #($n:expr) $name:ident($field:ty) $($req:tt)*) => (
        impl_deserialize!($impler, $(@$finished)* @($name $n) -- #($n + 1) $($req)*);
    );
    // Entry
    ($impler:ident, $($started:tt)*) => (impl_deserialize!($impler, -- #(0) $($started)*););
}

/// The main macro that creates RPC services.
///
/// Rpc methods are specified, mirroring trait syntax:
///
/// ```
/// # #[macro_use] extern crate tarpc;
/// # fn main() {}
/// # service! {
/// #[doc="Say hello"]
/// rpc hello(name: String) -> String;
/// # }
/// ```
///
/// There are two rpc names reserved for the default fns `spawn` and `spawn_with_config`.
///
/// Attributes can be attached to each rpc. These attributes
/// will then be attached to the generated `Service` trait's
/// corresponding method, as well as to the `Client` stub's rpcs methods.
///
/// The following items are expanded in the enclosing module:
///
/// * `Service` -- the trait defining the RPC service. It comes with two default methods for
///                starting the server:
///                1. `spawn` starts the service in another thread using default configuration.
///                2. `spawn_with_config` starts the service in another thread using the specified
///                   `Config`.
/// * `Client` -- a client that makes synchronous requests to the RPC server
/// * `FutureClient` -- a client that makes asynchronous requests to the RPC server
/// * `Future` -- a handle for asynchronously retrieving the result of an RPC
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
// Entry point
    (
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) $(-> $out:ty)*;
        )*
    ) => {
        service! {{
            $(
                $(#[$attr])*
                rpc $fn_name( $( $arg : $in_ ),* ) $(-> $out)*;
            )*
        }}
    };
// Pattern for when the next rpc has an implicit unit return type
    (
        {
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* );

            $( $unexpanded:tt )*
        }
        $( $expanded:tt )*
    ) => {
        service! {
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
        service! {
            { $( $unexpanded )* }

            $( $expanded )*

            $(#[$attr])*
            rpc $fn_name( $( $arg : $in_ ),* ) -> $out;
        }
    };
// Pattern for when all return types have been expanded
    (
        { } // none left to expand
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident ( $( $arg:ident : $in_:ty ),* ) -> $out:ty;
        )*
    ) => {
        struct __BlockingServer<S>(::std::sync::Arc<S>)
            where S: BlockingService + 'static;
        impl<S> ::std::clone::Clone for __BlockingServer<S>
            where S: BlockingService
        {
            fn clone(&self) -> Self {
                __BlockingServer(self.0.clone())
            }
        }

        impl<S> $crate::protocol::Service for __BlockingServer<S>
            where S: BlockingService + 'static
        {
            #[inline]
            fn handle(&mut self,
                      token: $crate::macros::mio::Token,
                      packet: $crate::protocol::Packet,
                      event_loop: &mut $crate::macros::mio::EventLoop<$crate::protocol::server::Dispatcher>)
            {
                let me = self.clone();
                let sender = event_loop.channel();
                ::std::thread::spawn(move || {
                    let result = match $crate::macros::bincode::deserialize_from(&mut ::std::io::Cursor::new(&packet.payload), $crate::macros::bincode::SizeLimit::Infinite).unwrap() {
                        $(
                            __ServerSideRequest::$fn_name(( $($arg,)* )) =>
                                __Reply::$fn_name((&*me.0).$fn_name($($arg),*)),
                        )*
                    };
                    // TODO(tikue): error handling!
                    let _ = sender.send($crate::protocol::server::Action::Reply(token, packet.reply(&result)));
                });
            }
        }

        #[allow(unused)]
        pub struct RequestContext {
            token: $crate::macros::mio::Token,
            request_id: u64,
            sender: $crate::macros::mio::Sender<$crate::protocol::server::Action>,
        }

        impl RequestContext {
            $(
                #[allow(unused)]
                #[inline]
                fn $fn_name(&self, result: $out) {
                    let result = __Reply::$fn_name(result);
                    // TODO(tikue): error handling
                    let _ = self.sender.send($crate::protocol::server::Action::Reply(self.token, $crate::protocol::Packet {
                        id: self.request_id,
                        payload: $crate::macros::bincode::serialize(&result, $crate::macros::bincode::SizeLimit::Infinite).unwrap()
                    }));
                }
            )*
        }

        #[doc="Defines the RPC service."]
        pub trait Service: Send {
            $(
                $(#[$attr])*
                #[inline]
                fn $fn_name(&mut self, context: RequestContext, $($arg:$in_),*);
            )*

            #[doc="Spawn a running service."]
            fn spawn<A>(self, addr: A) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      Self: ::std::marker::Sized + 'static,
            {
                $crate::protocol::Server::spawn(addr, __Server(self))
            }
        }

        struct __Server<S>(S)
            where S: Service;
        impl<S> $crate::protocol::Service for __Server<S>
            where S: Service
        {
            #[inline]
            fn handle(&mut self,
                      token: $crate::macros::mio::Token,
                      packet: $crate::protocol::Packet,
                      event_loop: &mut $crate::macros::mio::EventLoop<$crate::protocol::server::Dispatcher>)
            {
                match $crate::macros::bincode::deserialize_from(&mut ::std::io::Cursor::new(packet.payload), $crate::macros::bincode::SizeLimit::Infinite).unwrap() {
                    $(
                        __ServerSideRequest::$fn_name(( $($arg,)* )) =>
                            (self.0).$fn_name(RequestContext {
                                token: token,
                                request_id: packet.id,
                                sender: event_loop.channel(),
                            }, $($arg),*),
                    )*
                }
            }
        }

        #[doc="Defines the blocking RPC service."]
        pub trait BlockingService: ::std::marker::Send + ::std::marker::Sync + ::std::marker::Sized {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $out;
            )*

            #[doc="Spawn a running service."]
            fn spawn<A>(self, addr: A) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      Self: 'static,
            {
                $crate::protocol::Server::spawn(addr, __BlockingServer(::std::sync::Arc::new(self)))
            }
        }

        impl<P, S> BlockingService for P
            where P: ::std::marker::Send + ::std::marker::Sync + ::std::marker::Sized + 'static + ::std::ops::Deref<Target=S>,
                  S: BlockingService
        {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $out {
                    BlockingService::$fn_name(&**self, $($arg),*)
                }
            )*
        }

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum __ClientSideRequest<'a> {
            $(
                $fn_name(&'a ( $(&'a $in_,)* ))
            ),*
        }

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum __ServerSideRequest {
            $(
                $fn_name(( $($in_,)* ))
            ),*
        }

        impl_serialize!(__ClientSideRequest, { <'__a> }, $($fn_name(($($in_),*)))*);
        impl_deserialize!(__ServerSideRequest, $($fn_name(($($in_),*)))*);

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum __Reply {
            $(
                $fn_name($out),
            )*
        }

        impl_serialize!(__Reply, { }, $($fn_name($out))*);
        impl_deserialize!(__Reply, $($fn_name($out))*);

        #[allow(unused)]
        #[doc="An asynchronous RPC call"]
        pub struct Future<T> {
            future: $crate::Result<$crate::protocol::Future<__Reply>>,
            mapper: fn(__Reply) -> T,
        }

        impl<T> Future<T> {
            #[allow(unused)]
            #[doc="Block until the result of the RPC call is available"]
            pub fn get(self) -> $crate::Result<T> {
                try!(self.future).get().map(self.mapper)
            }
        }

        #[allow(unused)]
        #[doc="The client stub that makes RPC calls to the server."]
        pub struct Client($crate::protocol::ClientHandle);

        impl Client {
            #[allow(unused)]
            #[doc="Create a new client that communicates over the given socket."]
            pub fn spawn<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!($crate::protocol::Client::spawn(addr));
                ::std::result::Result::Ok(Client(inner))
            }

            #[allow(unused)]
            #[doc="Create a new client that connects via the given dialer."]
            pub fn dial(dialer: &$crate::transport::tcp::TcpDialer) -> $crate::Result<Self> {
                Client::spawn(&dialer.0)
            }

            #[allow(unused)]
            #[doc="Shuts down the event loop the client is running on."]
            pub fn shutdown(self) -> $crate::Result<()> {
                self.0.shutdown()
            }

            client_methods!(
                $(
                    { $(#[$attr])* }
                    $fn_name(($($arg,)*) : ($($in_,)*)) -> $out
                )*
            );
        }

        impl ::std::clone::Clone for Client {
            fn clone(&self) -> Self {
                Client(self.0.clone())
            }
        }

        #[allow(unused)]
        #[doc="The client stub that makes RPC calls to the server. Exposes a Future interface."]
        pub struct FutureClient($crate::protocol::ClientHandle);

        impl FutureClient {
            #[allow(unused)]
            #[doc="Create a new client that communicates over the given socket."]
            pub fn spawn<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!($crate::protocol::Client::spawn(addr));
                ::std::result::Result::Ok(FutureClient(inner))
            }

            #[allow(unused)]
            #[doc="Create a new client that connects via the given dialer."]
            pub fn dial(dialer: &$crate::transport::tcp::TcpDialer) -> $crate::Result<Self> {
                FutureClient::spawn(&dialer.0)
            }

            #[allow(unused)]
            #[doc="Shuts down the event loop the client is running on."]
            pub fn shutdown(self) -> $crate::Result<()> {
                self.0.shutdown()
            }

            async_client_methods!(
                $(
                    { $(#[$attr])* }
                    $fn_name(($($arg,)*): ($($in_,)*)) -> $out
                )*
            );

        }

        impl ::std::clone::Clone for FutureClient {
            fn clone(&self) -> Self {
                FutureClient(self.0.clone())
            }
        }
    }
}

#[allow(dead_code)] // because we're just testing that the macro expansion compiles
#[cfg(test)]
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
            rpc bi_fn(bar: String, baz: u64) -> String;
        }
    }
}

#[cfg(test)]
mod functional_test {
    extern crate env_logger;
    extern crate tempdir;

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
    }

    struct Server;

    impl BlockingService for Server {
        fn add(&self, x: i32, y: i32) -> i32 {
            x + y
        }
        fn hey(&self, name: String) -> String {
            format!("Hey, {}.", name)
        }
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let handle = Server.spawn("localhost:0").unwrap();
        let client = Client::spawn(handle.local_addr).unwrap();
        assert_eq!(3, client.add(&1, &2).unwrap());
        assert_eq!("Hey, Tim.", client.hey(&"Tim".into()).unwrap());
        client.shutdown().unwrap();
        handle.shutdown().unwrap();
    }

    #[test]
    fn simple_async() {
        let _ = env_logger::init();
        let handle = Server.spawn("localhost:0").unwrap();
        let client = FutureClient::spawn(handle.local_addr).unwrap();
        assert_eq!(3, client.add(&1, &2).get().unwrap());
        assert_eq!("Hey, Adam.", client.hey(&"Adam".into()).get().unwrap());
        client.shutdown().unwrap();
        handle.shutdown().unwrap();
    }

    #[test]
    fn clone() {
        let handle = Server.spawn("localhost:0").unwrap();
        let client1 = Client::spawn(handle.local_addr).unwrap();
        let client2 = client1.clone();
        assert_eq!(3, client1.add(&1, &2).unwrap());
        assert_eq!(3, client2.add(&1, &2).unwrap());
    }

    #[test]
    fn async_clone() {
        let handle = Server.spawn("localhost:0").unwrap();
        let client1 = FutureClient::spawn(handle.local_addr).unwrap();
        let client2 = client1.clone();
        assert_eq!(3, client1.add(&1, &2).get().unwrap());
        assert_eq!(3, client2.add(&1, &2).get().unwrap());
    }

    #[test]
    #[ignore = "Unix Sockets not yet supported by async client"]
    fn async_try_clone_unix() {
        /*
        let temp_dir = tempdir::TempDir::new("tarpc").unwrap();
        let temp_file = temp_dir.path()
                                .join("async_try_clone_unix.tmp");
        let handle = Server.spawn(UnixTransport(temp_file)).unwrap();
        let client1 = FutureClient::new(handle.dialer()).unwrap();
        let client2 = client1.clone();
        assert_eq!(3, client1.add(1, 2).get().unwrap());
        assert_eq!(3, client2.add(1, 2).get().unwrap());
        */
    }

    // Tests that a server can be wrapped in an Arc; no need to run, just compile
    #[allow(dead_code)]
    fn serve_arc_server() {
        let _ = ::std::sync::Arc::new(Server).spawn("localhost:0");
    }

    // Tests that a tcp client can be created from &str
    #[allow(dead_code)]
    fn test_client_str() {
        let _ = Client::spawn("localhost:0");
    }

    #[test]
    fn serde() {
        use bincode;
        let _ = env_logger::init();

        let to_add = (&1, &2);
        let request = __ClientSideRequest::add(&to_add);
        let ser = bincode::serde::serialize(&request, bincode::SizeLimit::Infinite).unwrap();
        let de = bincode::serde::deserialize(&ser).unwrap();
        if let __ServerSideRequest::add((1, 2)) = de {
            // success
        } else {
            panic!("Expected __ServerSideRequest::add, got {:?}", de);
        }
    }
}
