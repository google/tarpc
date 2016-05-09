// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

// Vendored from log crate so users don't need to do #[macro_use] extern crate log;
#[doc(hidden)]
#[macro_export]
macro_rules! __log {
    (target: $target:expr, $lvl:expr, $($arg:tt)+) => ({
        static _LOC: $crate::log::LogLocation = $crate::log::LogLocation {
            __line: line!(),
            __file: file!(),
            __module_path: module_path!(),
        };
        let lvl = $lvl;
        if lvl <= $crate::log::__static_max_level() && lvl <= $crate::log::max_log_level() {
            $crate::log::__log(lvl, $target, &_LOC, format_args!($($arg)+))
        }
    });
    ($lvl:expr, $($arg:tt)+) => (__log!(target: module_path!(), $lvl, $($arg)+))
}

// Vendored from log crate so users don't need to do #[macro_use] extern crate log;
#[doc(hidden)]
#[macro_export]
macro_rules! __error {
    (target: $target:expr, $($arg:tt)*) => (
        __log!(target: $target, $crate::log::LogLevel::Error, $($arg)*);
    );
    ($($arg:tt)*) => (
        __log!($crate::log::LogLevel::Error, $($arg)*);
    )
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
            impl$($lifetime)* $crate::serde::Serialize for $impler$($lifetime)* {
                #[inline]
                fn serialize<S>(&self, serializer: &mut S) -> ::std::result::Result<(), S::Error>
                    where S: $crate::serde::Serializer
                {
                    match *self {
                        $(
                            $impler::$name(ref field) =>
                                $crate::serde::Serializer::serialize_newtype_variant(
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
    ($impler:ident,
     { $($lifetime:tt)* },
     $(@$finished:tt)*
     -- #($n:expr) $name:ident($field:ty) $($req:tt)*) =>
    (
        impl_serialize!($impler,
                        { $($lifetime)* },
                        $(@$finished)* @($name $n)
                        -- #($n + 1) $($req)*);
    );
// Entry
    ($impler:ident,
     { $($lifetime:tt)* },
     $($started:tt)*) => (impl_serialize!($impler, { $($lifetime)* }, -- #(0) $($started)*););
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_deserialize {
    ($impler:ident, $(@($name:ident $n:expr))* -- #($_n:expr) ) => (
        impl $crate::serde::Deserialize for $impler {
            #[inline]
            fn deserialize<D>(deserializer: &mut D)
                -> ::std::result::Result<$impler, D::Error>
                where D: $crate::serde::Deserializer
            {
                #[allow(non_camel_case_types, unused)]
                enum __Field {
                    $($name),*
                }
                impl $crate::serde::Deserialize for __Field {
                    #[inline]
                    fn deserialize<D>(deserializer: &mut D)
                        -> ::std::result::Result<__Field, D::Error>
                        where D: $crate::serde::Deserializer
                    {
                        struct __FieldVisitor;
                        impl $crate::serde::de::Visitor for __FieldVisitor {
                            type Value = __Field;

                            #[inline]
                            fn visit_usize<E>(&mut self, value: usize)
                                -> ::std::result::Result<__Field, E>
                                where E: $crate::serde::de::Error,
                            {
                                $(
                                    if value == $n {
                                        return ::std::result::Result::Ok(__Field::$name);
                                    }
                                )*
                                return ::std::result::Result::Err(
                                    $crate::serde::de::Error::custom(
                                        format!("No variants have a value of {}!", value))
                                );
                            }
                        }
                        deserializer.deserialize_struct_field(__FieldVisitor)
                    }
                }

                struct __Visitor;
                impl $crate::serde::de::EnumVisitor for __Visitor {
                    type Value = $impler;

                    #[inline]
                    fn visit<__V>(&mut self, mut visitor: __V)
                        -> ::std::result::Result<$impler, __V::Error>
                        where __V: $crate::serde::de::VariantVisitor
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
/// /// Say hello
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
///                1. `spawn` starts a new event loop on another thread and registers the service
///                   on it.
///                2. `register` registers the service on an existing event loop.
///  * `BlockingService` -- a service trait that provides a more intuitive interface for when
///                         spawning a thread per request is acceptable.
/// * `Client` -- a client whose rpc functions each accept a callback invoked when the response is
///               available.
/// * `FutureClient` -- a client whose rpc functions return futures, a thin wrapper around
///                     channels. Useful for scatter/gather-type actions.
/// * `BlockingClient` -- a client whose rpc functions block until the reply is available. Easiest
///                       interface to use, as it looks the same as a regular function call.
/// * `Future` -- a thin wrapper around a channel for retrieving the result of an RPC.
/// * `Ctx` -- the server request context which is called when the reply is ready. Is not `Send`,
///            but is useful when replies are available immediately, as it doesn't have to send
///            a notification to the event loop and so might be a bit faster.
/// * `SendCtx` -- like `Ctx`, but can be sent across threads. `Ctx` can be converted to `SendCtx`
///                via the `sendable` fn.
///
/// **Warning**: In addition to the above items, there are a few expanded items that
/// are considered implementation details. As with the above items, shadowing
/// these item names in the enclosing module is likely to break things in confusing
/// ways:
///
/// * `__Server`
/// * `__BlockingServer`
/// * `__ClientSideRequest`
/// * `__ServerSideRequest`
///
/// Additionally, it is best to not define rpcs with the following names, so as to avoid conflicts
/// with fns included on the client, service, and ctx:
///
/// * `sendable`
/// * `register`
/// * `spawn`
/// * `shutdown`
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
                      connection: &mut $crate::protocol::server::ClientConnection,
                      packet: $crate::protocol::Packet,
                      event_loop: &mut $crate::mio::EventLoop<$crate::protocol::server::Dispatcher>)
            {
                let me = self.clone();
                let token = connection.token();
                let sender = event_loop.channel();
                ::std::thread::spawn(move || {
                    let request = match $crate::protocol::deserialize(&packet.payload) {
                        Ok(request) => request,
                        Err(e) => {
                            __error!("Service {:?}: failed to deserialize request packet {:?},
                                     {:?}", token, packet.id, e);
                            return;
                        }
                    };
                    let result = match request {
                        $(
                            __ServerSideRequest::$fn_name(( $($arg,)* )) =>
                                $crate::protocol::serialize(&(&*me.0).$fn_name($($arg),*)),
                        )*
                    };
                    let result = match result {
                        Ok(result) => result,
                        Err(e) => {
                            __error!("Service {:?}: failed to serialize reply packet {:?}, {:?}",
                                     token, packet.id, e);
                            return;
                        }
                    };
                    let reply = $crate::protocol::Packet {
                        id: packet.id,
                        payload: result,
                    };
// TODO(tikue): error handling!
                    let _ = sender.send($crate::protocol::server::Action::Reply(token, reply));
                });
            }
        }

/// The request context by which replies are sent.
        #[allow(unused)]
        pub struct Ctx<'a> {
            request_id: u64,
            connection: &'a mut $crate::protocol::server::ClientConnection,
            event_loop: &'a mut $crate::mio::EventLoop<$crate::protocol::server::Dispatcher>,
        }

        impl<'a> Ctx<'a> {
            #[allow(unused)]
            #[inline]
            /// Convert the context into a version that can be sent across threads.
            pub fn sendable(&self) -> SendCtx {
                SendCtx {
                    request_id: self.request_id,
                    token: self.connection.token(),
                    tx: self.event_loop.channel(),
                }
            }

            $(
                #[allow(unused)]
                #[inline]
                /// Replies to the rpc with the same name.
                pub fn $fn_name(self, result: &$out) -> $crate::Result<()> {
                    self.connection.reply(self.event_loop, $crate::protocol::Packet {
                        id: self.request_id,
                        payload: try!($crate::protocol::serialize(&result))
                    });
                    return Ok(())
                }
            )*
        }

        /// The request context by which replies are sent. Same as `Ctx` but can be sent across
        /// threads.
        #[allow(unused)]
        pub struct SendCtx {
            request_id: u64,
            token: $crate::mio::Token,
            tx: $crate::mio::Sender<$crate::protocol::server::Action>,
        }

        impl SendCtx {
            $(
                #[allow(unused)]
                #[inline]
                /// Replies to the rpc with the same name.
                pub fn $fn_name(self, result: &$out) -> $crate::Result<()> {
                    let reply = $crate::protocol::server::Action::Reply(self.token,
                                                                        $crate::protocol::Packet {
                        id: self.request_id,
                        payload: try!($crate::protocol::serialize(&result)),
                    });
                    try!(self.tx.send(reply));
                    Ok(())
                }
            )*
        }

        /// Defines the RPC service.
        pub trait Service: Send {
            $(
                $(#[$attr])*
                #[inline]
                #[allow(unused)]
                fn $fn_name(&mut self, context: Ctx, $($arg:$in_),*);
            )*

            #[allow(unused)]
            /// Spawn a running service on a new event loop.
            fn spawn<A>(self, addr: A) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      Self: ::std::marker::Sized + 'static,
            {
                $crate::protocol::Server::spawn(addr, __Server(self))
            }

            #[allow(unused)]
/// Spawn a running service.
            fn register<A>(self, addr: A, registry: &$crate::protocol::server::Registry)
                -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      Self: ::std::marker::Sized + 'static
            {
                registry.clone().register(try!($crate::protocol::Server::new(addr, __Server(self))))
            }
        }

        struct __Server<S>(S)
            where S: Service;
        impl<S> $crate::protocol::Service for __Server<S>
            where S: Service
        {
            #[inline]
            fn handle(&mut self,
                      connection: &mut $crate::protocol::server::ClientConnection,
                      packet: $crate::protocol::Packet,
                      event_loop: &mut $crate::mio::EventLoop<$crate::protocol::server::Dispatcher>)
            {
                let request = match $crate::protocol::deserialize(&packet.payload) {
                    Ok(request) => request,
                    Err(e) => {
                        __error!("Service {:?}: failed to deserialize request packet {:?}, {:?}",
                                 connection.token(), packet.id, e);
                        return;
                    }
                };
                match request {
                    $(
                        __ServerSideRequest::$fn_name(( $($arg,)* )) =>
                            (self.0).$fn_name(Ctx {
                                request_id: packet.id,
                                connection: connection,
                                event_loop: event_loop,
                            }, $($arg),*),
                    )*
                }
            }
        }

        /// Defines the blocking RPC service.
        pub trait BlockingService
            : ::std::marker::Send + ::std::marker::Sync + ::std::marker::Sized {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $out;
            )*

            /// Spawn a running service.
            fn spawn<A>(self, addr: A) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      Self: 'static,
            {
                $crate::protocol::Server::spawn(addr, __BlockingServer(::std::sync::Arc::new(self)))
            }
        }

        impl<P, S> BlockingService for P
            where P: ::std::marker::Send +
                     ::std::marker::Sync +
                     ::std::marker::Sized +
                     'static +
                     ::std::ops::Deref<Target=S>,
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

        #[allow(unused)]
        /// An asynchronous RPC call
        pub struct Future<T>
            where T: $crate::serde::Deserialize
        {
            future: $crate::Result<$crate::protocol::Future<T>>,
            mapper: fn(T) -> T,
        }

        impl<T> Future<T>
            where T: $crate::serde::Deserialize
        {
            #[allow(unused)]
            /// Block until the result of the RPC call is available
            pub fn get(self) -> $crate::Result<T> {
                try!(self.future).get().map(self.mapper)
            }
        }

        #[allow(unused)]
        /// The client stub that makes RPC calls to the server.
        pub struct Client($crate::protocol::ClientHandle);

        impl Client {
            #[allow(unused)]
            /// Create a new client that communicates over the given socket.
            pub fn spawn<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!($crate::protocol::Client::spawn(addr));
                ::std::result::Result::Ok(Client(inner))
            }

            #[allow(unused)]
            /// Register a new client that communicates over the given socket.
            pub fn register<A>(addr: A, register: &$crate::protocol::client::Registry)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!(register.register(addr));
                ::std::result::Result::Ok(Client(inner))
            }

            #[allow(unused)]
            /// Shuts down the event loop the client is running on.
            pub fn shutdown(self) -> $crate::Result<()> {
                self.0.shutdown()
            }

            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name<__F>(&self, __f: __F, $($arg: &$in_),*) -> $crate::Result<()>
                    where __F: FnOnce($crate::Result<$out>) + Send + 'static
                {
                    (self.0).rpc(&__ClientSideRequest::$fn_name(&($($arg,)*)), __f)
                }
            )*
        }

        #[allow(unused)]
/// The client stub that makes RPC calls to the server.
        pub struct BlockingClient($crate::protocol::ClientHandle);

        impl BlockingClient {
            #[allow(unused)]
/// Create a new client that communicates over the given socket.
            pub fn spawn<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!($crate::protocol::Client::spawn(addr));
                ::std::result::Result::Ok(BlockingClient(inner))
            }

            #[allow(unused)]
/// Register a new client that communicates over the given socket.
            pub fn register<A>(addr: A, register: &$crate::protocol::client::Registry)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!(register.register(addr));
                ::std::result::Result::Ok(BlockingClient(inner))
            }

            #[allow(unused)]
/// Shuts down the event loop the client is running on.
            pub fn shutdown(self) -> $crate::Result<()> {
                self.0.shutdown()
            }

            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*) -> $crate::Result<$out> {
                    try!((self.0).rpc_fut(&__ClientSideRequest::$fn_name(&($($arg,)*)))).get()
                }
            )*
        }

        impl ::std::clone::Clone for BlockingClient {
            fn clone(&self) -> Self {
                BlockingClient(self.0.clone())
            }
        }

        #[allow(unused)]
/// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct FutureClient($crate::protocol::ClientHandle);

        impl FutureClient {
            #[allow(unused)]
/// Create a new client that communicates over the given socket.
            pub fn spawn<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!($crate::protocol::Client::spawn(addr));
                ::std::result::Result::Ok(FutureClient(inner))
            }

            #[allow(unused)]
/// Shuts down the event loop the client is running on.
            pub fn shutdown(self) -> $crate::Result<()> {
                self.0.shutdown()
            }

            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*) -> Future<$out> {
                    fn mapper(reply: $out) -> $out {
                        reply
                    }
                    let reply = (self.0).rpc_fut(&__ClientSideRequest::$fn_name(&($($arg,)*)));
                    Future {
                        future: reply,
                        mapper: mapper,
                    }
                }
            )*

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
        let client = BlockingClient::spawn(handle.local_addr()).unwrap();
        assert_eq!(3, client.add(&1, &2).unwrap());
        assert_eq!("Hey, Tim.", client.hey(&"Tim".into()).unwrap());
        client.shutdown().unwrap();
        handle.shutdown().unwrap();
    }

    #[test]
    fn simple_async() {
        let _ = env_logger::init();
        let handle = Server.spawn("localhost:0").unwrap();
        let client = FutureClient::spawn(handle.local_addr()).unwrap();
        assert_eq!(3, client.add(&1, &2).get().unwrap());
        assert_eq!("Hey, Adam.", client.hey(&"Adam".into()).get().unwrap());
        client.shutdown().unwrap();
        handle.shutdown().unwrap();
    }

    #[test]
    fn clone() {
        let handle = Server.spawn("localhost:0").unwrap();
        let client1 = BlockingClient::spawn(handle.local_addr()).unwrap();
        let client2 = client1.clone();
        assert_eq!(3, client1.add(&1, &2).unwrap());
        assert_eq!(3, client2.add(&1, &2).unwrap());
    }

    #[test]
    fn async_clone() {
        let handle = Server.spawn("localhost:0").unwrap();
        let client1 = FutureClient::spawn(handle.local_addr()).unwrap();
        let client2 = client1.clone();
        assert_eq!(3, client1.add(&1, &2).get().unwrap());
        assert_eq!(3, client2.add(&1, &2).get().unwrap());
    }

    #[test]
    #[ignore = "Unix Sockets not yet supported by async client"]
    fn async_try_clone_unix() {
        // let temp_dir = tempdir::TempDir::new("tarpc").unwrap();
        // let temp_file = temp_dir.path()
        // .join("async_try_clone_unix.tmp");
        // let handle = Server.spawn(UnixTransport(temp_file)).unwrap();
        // let client1 = FutureClient::new(handle.dialer()).unwrap();
        // let client2 = client1.clone();
        // assert_eq!(3, client1.add(1, 2).get().unwrap());
        // assert_eq!(3, client2.add(1, 2).get().unwrap());
        //
    }

    // Tests that a server can be wrapped in an Arc; no need to run, just compile
    #[allow(dead_code)]
    fn serve_arc_server() {
        let _ = ::std::sync::Arc::new(Server).spawn("localhost:0");
    }

    // Tests that a tcp client can be created from &str
    #[allow(dead_code)]
    fn test_client_str() {
        let _ = BlockingClient::spawn("localhost:0");
    }

    #[test]
    fn serde() {
        let _ = env_logger::init();

        let to_add = (&1, &2);
        let request = __ClientSideRequest::add(&to_add);
        let ser = ::protocol::serialize(&request).unwrap();
        let de = ::protocol::deserialize(&ser).unwrap();
        if let __ServerSideRequest::add((1, 2)) = de {
            // success
        } else {
            panic!("Expected __ServerSideRequest::add, got {:?}", de);
        }
    }
}