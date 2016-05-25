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


#[doc(hidden)]
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
                enum Field {
                    $($name),*
                }
                impl $crate::serde::Deserialize for Field {
                    #[inline]
                    fn deserialize<D>(deserializer: &mut D)
                        -> ::std::result::Result<Field, D::Error>
                        where D: $crate::serde::Deserializer
                    {
                        struct FieldVisitor;
                        impl $crate::serde::de::Visitor for FieldVisitor {
                            type Value = Field;

                            #[inline]
                            fn visit_usize<E>(&mut self, value: usize)
                                -> ::std::result::Result<Field, E>
                                where E: $crate::serde::de::Error,
                            {
                                $(
                                    if value == $n {
                                        return ::std::result::Result::Ok(Field::$name);
                                    }
                                )*
                                ::std::result::Result::Err(
                                    $crate::serde::de::Error::custom(
                                        format!("No variants have a value of {}!", value))
                                )
                            }
                        }
                        deserializer.deserialize_struct_field(FieldVisitor)
                    }
                }

                struct Visitor;
                impl $crate::serde::de::EnumVisitor for Visitor {
                    type Value = $impler;

                    #[inline]
                    fn visit<V>(&mut self, mut visitor: V)
                        -> ::std::result::Result<$impler, V::Error>
                        where V: $crate::serde::de::VariantVisitor
                    {
                        match try!(visitor.visit_variant()) {
                            $(
                                Field::$name => {
                                    let val = try!(visitor.visit_newtype());
                                    ::std::result::Result::Ok($impler::$name(val))
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
                deserializer.deserialize_enum(stringify!($impler), VARIANTS, Visitor)
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
/// # #![feature(default_type_parameter_fallback)]
/// # #[macro_use] extern crate tarpc;
/// # fn main() {}
/// # service! {
/// /// Say hello
/// rpc hello(name: String) -> String;
/// # }
/// ```
///
/// There are two rpc names reserved for the default fns `listen` and `listen_with_config`.
///
/// Attributes can be attached to each rpc. These attributes
/// will then be attached to the generated `AsyncService` trait's
/// corresponding method, as well as to the `AsyncClient` stub's rpcs methods.
///
/// The following items are expanded in the enclosing module:
///
/// * `AsyncService` -- the trait defining the RPC service.
/// * `SyncService` -- a service trait that provides a more intuitive interface for when
///                     listening a thread per request is acceptable. `SyncService`s must be
///                     clone, so that the service can run in multiple threads, and they must
///                     have a 'static lifetime, so that the service can outlive the threads
///                     it runs in. For stateful services, typically this means impling
///                     `SyncService` for `Arc<YourService>`. All `SyncService`s automatically
///                     `impl AsyncService`.
/// * `ServiceExt` -- provides the methods for starting a service. An umbrella impl is provided
///                    for all implers of `AsyncService`. The methods provided are:
///                1. `listen` starts a new event loop on another thread and registers the service
///                   on it.
///                2. `register` registers the service on an existing event loop.
/// * `AsyncClient` -- a client whose rpc functions each accept a callback invoked when the
///                    response is available. The callback argument is named `__f`, so
///                    naming an rpc argument the same will likely not work.
/// * `FutureClient` -- a client whose rpc functions return futures, a thin wrapper around
///                     channels. Useful for scatter/gather-type actions.
/// * `SyncClient` -- a client whose rpc functions block until the reply is available. Easiest
///                       interface to use, as it looks the same as a regular function call.
///
/// **Warning**: In addition to the above items, there are a few expanded items that
/// are considered implementation details. As with the above items, shadowing
/// these item names in the enclosing module is likely to break things in confusing
/// ways:
///
/// * `__ClientSideRequest`
/// * `__ServerSideRequest`
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

        /// Defines the RPC service.
        pub trait AsyncService: ::std::marker::Send + ::std::marker::Sized + 'static {
            $(
                $(#[$attr])*
                /// When the reply is ready, send it to the client via `tarpc::Ctx::reply`.
                #[inline]
                #[allow(unused)]
                fn $fn_name(&mut self, $crate::Ctx<$out>, $($arg:$in_),*);
            )*
        }

        pub trait SyncServiceExt: SyncService {
            /// Registers the service with the global registry, listening on the given address.
            fn listen<A>(self, addr: A) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
            {
                SyncServiceExt::register(self, addr, $crate::server::Config::default())
            }

            /// Registers the service with the given registry, listening on the given address.
            fn register<A>(self, addr: A, config: $crate::server::Config)
                -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
            {
                return AsyncServiceExt::register(__SyncServer {
                    thread_pool: $crate::cached_pool::CachedPool::new($crate::cached_pool::Config {
                        max_threads: config.max_requests,
                        min_threads: config.min_threads,
                        max_idle: config.thread_max_idle,
                    }),
                    service: self,
                }, addr, config);

                #[derive(Clone)]
                struct __SyncServer<S> {
                    thread_pool: $crate::cached_pool::CachedPool,
                    service: S,
                }

                impl<S> AsyncService for __SyncServer<S> where S: SyncService {
                    $(
                        fn $fn_name(&mut self, ctx: $crate::Ctx<$out>, $($arg:$in_),*) {
                            let send_ctx = ctx.sendable();
                            let service = self.service.clone();
                            if let ::std::result::Result::Err(_) = self.thread_pool.execute(
                                move || {
                                    let reply = service.$fn_name($($arg),*);
                                    let token = send_ctx.connection_token();
                                    let id = send_ctx.request_id();
                                    if let ::std::result::Result::Err(e) =
                                        send_ctx.reply(reply)
                                    {
                                        __error!("SyncService {:?}: failed to send reply {:?}, \
                                                 {:?}", token, id, e);
                                    }
                                }
                            ) {
                                let token = ctx.connection_token();
                                let id = ctx.request_id();
                                if let ::std::result::Result::Err(e) =
                                    ctx.reply(::std::result::Result::Err($crate::Error::Busy)) {
                                    __error!("SyncService {:?}: failed to send reply {:?}, {:?}",
                                             token, id, e);
                                }
                            }
                        }
                    )*
                }
            }
        }

        /// Provides methods for starting the service.
        pub trait AsyncServiceExt: AsyncService {
            /// Registers the service with the global registry, listening on the given address.
            fn listen<A>(self, addr: A) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
            {
                self.register(addr, $crate::server::Config::default())
            }

            /// Registers the service with the given registry, listening on the given address.
            fn register<A>(self, addr: A, config: $crate::server::Config)
                -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
            {
                return config.registry.register(
                    try!($crate::protocol::AsyncServer::configured(addr,
                                                                   __AsyncServer(self),
                                                                   &config)));

                struct __AsyncServer<S>(S);

                impl<S> ::std::fmt::Debug for __AsyncServer<S> {
                    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(fmt, "__AsyncServer")
                    }
                }

                impl<S> $crate::protocol::AsyncService for __AsyncServer<S>
                    where S: AsyncService
                {
                    #[inline]
                    fn handle(&mut self, ctx: $crate::GenericCtx, request: ::std::vec::Vec<u8>) {
                        let request = match $crate::protocol::deserialize(&request) {
                            ::std::result::Result::Ok(request) => request,
                            ::std::result::Result::Err(e) => {
                                other_service(ctx, e);
                                return;
                            }
                        };
                        match request {
                            $(
                                __ServerSideRequest::$fn_name(( $($arg,)* )) =>
                                    (self.0).$fn_name(ctx.for_type(), $($arg),*),
                            )*
                        }

                        #[inline]
                        fn other_service(ctx: $crate::GenericCtx, e: $crate::Error) {
                            __error!("AsyncServer {:?}: failed to deserialize request \
                                     packet {:?}, {:?}",
                                     ctx.connection_token(), ctx.request_id(), e);
                            let err: ::std::result::Result<(), _> = ::std::result::Result::Err(
                                $crate::CanonicalRpcError {
                                    code: $crate::CanonicalRpcErrorCode::WrongService,
                                    description:
                                        format!("Failed to deserialize request packet: {}", e)
                                });
                            let token = ctx.connection_token();
                            let id = ctx.request_id();
                            if let ::std::result::Result::Err(e) = ctx.reply(err) {
                                __error!("AsyncServer {:?}: failed to serialize \
                                         reply packet {:?}, {:?}", token, id, e);
                            }
                        }
                    }
                }

            }
        }

        impl<A> AsyncServiceExt for A where A: AsyncService {}
        impl<S> SyncServiceExt for S where S: SyncService {}


        /// Defines the blocking RPC service.
        pub trait SyncService: ::std::marker::Send + ::std::clone::Clone + 'static {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $crate::RpcResult<$out>;
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
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a callback interface.
        pub struct AsyncClient($crate::protocol::ClientHandle);

        impl $crate::Client for AsyncClient {
            #[allow(unused)]
            fn connect<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                Self::register(addr, &*$crate::protocol::client::REGISTRY)
            }

            #[allow(unused)]
            fn register<A>(addr: A, register: &$crate::protocol::client::Registry)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                let inner = try!(register.clone().register(addr));
                ::std::result::Result::Ok(AsyncClient(inner))
            }

        }
        impl AsyncClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                ///
                /// When the server's reply is available, or an error occurs, the given
                /// callback `__f` is invoked with the reply or error as argument.
                #[inline]
                pub fn $fn_name<__F>(&self, __f: __F, $($arg: &$in_),*) -> $crate::Result<()>
                    where __F: FnOnce($crate::Result<$out>) + Send + 'static
                {
                    (self.0).rpc(&__ClientSideRequest::$fn_name(&($($arg,)*)), __f)
                }
            )*
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a blocking interface.
        pub struct SyncClient(AsyncClient);

        impl $crate::Client for SyncClient {
            #[allow(unused)]
            fn connect<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                Self::register(addr, &*$crate::protocol::client::REGISTRY)
            }

            #[allow(unused)]
            fn register<A>(addr: A, registry: &$crate::protocol::client::Registry)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                ::std::result::Result::Ok(SyncClient(try!(AsyncClient::register(addr, registry))))
            }
        }

        impl SyncClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*) -> $crate::Result<$out> {
                    let (tx, rx) = ::std::sync::mpsc::channel();
                    (self.0).$fn_name(move |result| tx.send(result).unwrap(), $($arg),*);
                    rx.recv().unwrap()
                }
            )*
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct FutureClient(AsyncClient);

        impl $crate::Client for FutureClient {
            #[allow(unused)]
            fn connect<A>(addr: A) -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                Self::register(addr, &*$crate::protocol::client::REGISTRY)
            }

            #[allow(unused)]
            fn register<A>(addr: A, registry: &$crate::protocol::client::Registry)
                -> $crate::Result<Self>
                where A: ::std::net::ToSocketAddrs
            {
                ::std::result::Result::Ok(FutureClient(try!(AsyncClient::register(addr, registry))))
            }
        }

        impl FutureClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*) -> $crate::Future<$out> {
                    let (tx, rx) = ::std::sync::mpsc::channel();
                    (self.0).$fn_name(move |result| {
                        if let ::std::result::Result::Err(e) = tx.send(result) {
                            __error!("FutureClient: failed to send rpc reply, {:?}", e);
                        }
                    }, $($arg),*);
                    $crate::Future::new(rx)
                }
            )*

        }
    }
}

#[allow(dead_code)]
// because we're just testing that the macro expansion compiles
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
    use {Client, Ctx};
    extern crate env_logger;
    extern crate tempdir;

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
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

    mod blocking {
        use Client;
        use super::env_logger;
        use super::{FutureClient, SyncClient, SyncService, SyncServiceExt};
        use RpcResult;

        #[derive(Clone, Copy)]
        struct Server;

        impl SyncService for Server {
            fn add(&self, x: i32, y: i32) -> RpcResult<i32> {
                Ok(x + y)
            }
            fn hey(&self, name: String) -> RpcResult<String> {
                Ok(format!("Hey, {}.", name))
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = SyncClient::connect(handle.local_addr()).unwrap();
            assert_eq!(3, client.add(&1, &2).unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".into()).unwrap());
        }

        #[test]
        fn simple_async() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = FutureClient::connect(handle.local_addr()).unwrap();
            assert_eq!(3, client.add(&1, &2).get().unwrap());
            assert_eq!("Hey, Adam.", client.hey(&"Adam".into()).get().unwrap());
        }

        #[test]
        fn clone() {
            let handle = Server.listen("localhost:0").unwrap();
            let client1 = SyncClient::connect(handle.local_addr()).unwrap();
            let client2 = client1.clone();
            assert_eq!(3, client1.add(&1, &2).unwrap());
            assert_eq!(3, client2.add(&1, &2).unwrap());
        }

        #[test]
        fn async_clone() {
            let handle = Server.listen("localhost:0").unwrap();
            let client1 = FutureClient::connect(handle.local_addr()).unwrap();
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
            // let handle = Server.listen(UnixTransport(temp_file)).unwrap();
            // let client1 = FutureClient::new(handle.dialer()).unwrap();
            // let client2 = client1.clone();
            // assert_eq!(3, client1.add(1, 2).get().unwrap());
            // assert_eq!(3, client2.add(1, 2).get().unwrap());
            //
        }

        // Tests that a tcp client can be created from &str
        #[allow(dead_code)]
        fn test_client_str() {
            let _ = SyncClient::connect("localhost:0");
        }

        #[test]
        fn other_service() {
            let handle = Server.listen("localhost:0").unwrap();
            let client = super::other_service::SyncClient::connect(handle.local_addr()).unwrap();
            match client.foo().err().unwrap() {
                ::Error::WrongService(..) => {} // good
                bad => panic!("Expected RpcError(WrongService) but got {}", bad),
            }
        }
    }

    mod nonblocking {
        use {Client, Ctx};
        use super::env_logger;
        use super::{AsyncService, AsyncServiceExt, FutureClient, SyncClient};

        struct Server;

        impl AsyncService for Server {
            fn add(&mut self, ctx: Ctx<i32>, x: i32, y: i32) {
                ctx.reply(Ok(&(x + y))).unwrap();
            }
            fn hey(&mut self, ctx: Ctx<String>, name: String) {
                ctx.reply(Ok(&format!("Hey, {}.", name))).unwrap();
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = SyncClient::connect(handle.local_addr()).unwrap();
            assert_eq!(3, client.add(&1, &2).unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".into()).unwrap());
        }

        #[test]
        fn simple_async() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = FutureClient::connect(handle.local_addr()).unwrap();
            assert_eq!(3, client.add(&1, &2).get().unwrap());
            assert_eq!("Hey, Adam.", client.hey(&"Adam".into()).get().unwrap());
        }

        #[test]
        fn clone() {
            let handle = Server.listen("localhost:0").unwrap();
            let client1 = SyncClient::connect(handle.local_addr()).unwrap();
            let client2 = client1.clone();
            assert_eq!(3, client1.add(&1, &2).unwrap());
            assert_eq!(3, client2.add(&1, &2).unwrap());
        }

        #[test]
        fn async_clone() {
            let handle = Server.listen("localhost:0").unwrap();
            let client1 = FutureClient::connect(handle.local_addr()).unwrap();
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
            // let handle = Server.listen(UnixTransport(temp_file)).unwrap();
            // let client1 = FutureClient::new(handle.dialer()).unwrap();
            // let client2 = client1.clone();
            // assert_eq!(3, client1.add(1, 2).get().unwrap());
            // assert_eq!(3, client2.add(1, 2).get().unwrap());
            //
        }

        // Tests that a tcp client can be created from &str
        #[allow(dead_code)]
        fn test_client_str() {
            let _ = SyncClient::connect("localhost:0");
        }

        #[test]
        fn other_service() {
            let handle = Server.listen("localhost:0").unwrap();
            let client = super::other_service::SyncClient::connect(handle.local_addr()).unwrap();
            match client.foo().err().unwrap() {
                ::Error::WrongService(..) => {} // good
                bad => panic!("Expected RpcError(WrongService) but got {}", bad),
            }
        }
    }

    pub mod error_service {
        service! {
            rpc bar() -> u32;
        }
    }

    struct ErrorServer;
    impl error_service::AsyncService for ErrorServer {
        fn bar(&mut self, ctx: Ctx<u32>) {
            ctx.reply(::std::result::Result::Err(::RpcError {
                    code: ::RpcErrorCode::BadRequest,
                    description: "lol jk".to_string(),
                }))
                .unwrap();
        }
    }

    #[test]
    fn error() {
        use self::error_service::*;
        let _ = env_logger::init();

        let handle = ErrorServer.listen("localhost:0").unwrap();

        let client = AsyncClient::connect(handle.local_addr()).unwrap();
        let (tx, rx) = ::std::sync::mpsc::channel();
        client.bar(move |result| {
                match result.err().unwrap() {
                    ::Error::Rpc(::RpcError { code: ::RpcErrorCode::BadRequest, .. }) => {
                        tx.send(()).unwrap()
                    } // good
                    bad => panic!("Expected RpcError(BadRequest) but got {:?}", bad),
                }
            })
            .unwrap();
        rx.recv().unwrap();

        let client = SyncClient::connect(handle.local_addr()).unwrap();
        match client.bar().err().unwrap() {
            ::Error::Rpc(::RpcError { code: ::RpcErrorCode::BadRequest, .. }) => {} // good
            bad => panic!("Expected RpcError(BadRequest) but got {:?}", bad),
        }
    }

    pub mod other_service {
        service! {
            rpc foo();
        }
    }

    #[test]
    fn retry() {
        use self::other_service::{AsyncServiceExt, SyncClient};
        let _ = env_logger::init();

        let server = FailOnce(true).listen("localhost:0").unwrap();
        let client = SyncClient::connect(server.local_addr()).unwrap();
        client.foo().unwrap();


        struct FailOnce(bool);
        impl other_service::AsyncService for FailOnce {
            fn foo(&mut self, ctx: ::Ctx<()>) {
                if self.0 {
                    ctx.reply(Err(::Error::Busy)).unwrap();
                } else {
                    ctx.reply(Ok(())).unwrap();
                }
                self.0 = !self.0;
            }
        }
    }
}
