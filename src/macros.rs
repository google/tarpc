// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

/// Creates an enum where each variant contains a `Future`. The created enum impls `Future`.
/// Useful when a fn needs to return possibly many different types of futures.
#[macro_export]
macro_rules! future_enum {
    {
        $(#[$attr:meta])*
        enum $name:ident<$($tp:ident),*> {
            $(#[$attrv:meta])*
            $($variant:ident($inner:ty)),*
        }
    } => {
        $(#[$attr])*
        enum $name<$($tp),*> {
            $(#[$attrv])*
            $($variant($inner)),*
        }

        impl<T, $($tp),*> $crate::futures::Future for $name<$($tp),*>
            where T: Send + 'static,
                  $($inner: $crate::futures::Future<Item=T, Error=$crate::Error>),*
        {
            type Item = T;
            type Error = $crate::Error;

            fn poll(&mut self) -> $crate::futures::Poll<T, $crate::Error> {
                match *self {
                    $($name::$variant(ref mut f) => $crate::futures::Future::poll(f)),*
                }
            }
        }
    }
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
/// # #![feature(conservative_impl_trait)]
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
///                     have a `'static` lifetime, so that the service can outlive the threads
///                     it runs in. For stateful services, typically this means impling
///                     `SyncService` for `Arc<YourService>`. All `SyncService`s automatically
///                     `impl AsyncService`.
/// * `ServiceExt` -- provides the methods for starting a service. An umbrella impl is provided
///                    for all implers of `AsyncService`. The methods provided are:
///  1. `listen` starts a new event loop on another thread and registers the service
///     on it.
///  2. `register` registers the service on an existing event loop.
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
        pub trait FutureService:
            ::std::marker::Send +
            ::std::clone::Clone +
            'static
        {
            $(
                $(#[$attr])*
                #[inline]
                #[allow(unused)]
                fn $fn_name(&self, $($arg:$in_),*) -> $crate::Future<$out>;
            )*
        }

        pub trait SyncServiceExt: SyncService {
            /// Registers the service with the given registry, listening on the given address.
            fn listen<L>(self, addr: L)
                -> $crate::Result<$crate::tokio_proto::server::ServerHandle>
                where L: ::std::net::ToSocketAddrs
            {

                let service = __SyncServer {
                    service: self,
                };
                return service.listen(addr);

                #[derive(Clone)]
                struct __SyncServer<S> {
                    service: S,
                }

                impl<S> FutureService for __SyncServer<S> where S: SyncService {
                    $(
                        fn $fn_name(&self, $($arg:$in_),*) -> $crate::Future<$out> {
                            let (c, p) = $crate::futures::oneshot();
                            let service = self.clone();
                            ::std::thread::spawn(move || {
                                let reply = SyncService::$fn_name(&service.service, $($arg),*);
                                c.complete($crate::futures::IntoFuture::into_future(reply));
                            });
                            let p = $crate::futures::Future::map_err(p, |_| -> $crate::Error {
                                // TODO(tikue): what do do if SyncService panics?
                                unimplemented!()
                            });
                            $crate::futures::Future::boxed($crate::futures::Future::flatten(p))
                        }
                    )*
                }
            }
        }

        /// Provides methods for starting the service.
        pub trait FutureServiceExt: FutureService {
            /// Registers the service with the given registry, listening on the given address.
            fn listen<L>(self, addr: L)
                -> $crate::Result<$crate::tokio_proto::server::ServerHandle>
                where L: ::std::net::ToSocketAddrs
            {
                return $crate::Server.listen(addr, __AsyncServer(self));

                #[derive(Clone)]
                struct __AsyncServer<S>(S);

                impl<S> ::std::fmt::Debug for __AsyncServer<S> {
                    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(fmt, "__AsyncServer {{ .. }}")
                    }
                }

                #[allow(non_camel_case_types)]
                enum Reply {
                    DeserializeError($crate::SerializeFuture),
                    $($fn_name($crate::futures::Then<$crate::Future<$out>,
                                                     $crate::SerializeFuture,
                                                     fn($crate::Result<$out>)
                                                         -> $crate::SerializeFuture>)),*
                }

                impl $crate::futures::Future for Reply {
                    type Item = $crate::SerializedReply;
                    type Error = $crate::Error;

                    fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error>
                    {
                        match *self {
                            Reply::DeserializeError(ref mut f) => $crate::futures::Future::poll(f),
                            $(Reply::$fn_name(ref mut f) => $crate::futures::Future::poll(f)),*
                        }
                    }
                }


                impl<S> $crate::tokio_service::Service for __AsyncServer<S>
                    where S: FutureService
                {
                    type Req = ::std::vec::Vec<u8>;
                    type Resp = $crate::SerializedReply;
                    type Error = $crate::Error;
                    type Fut = Reply;

                    fn call(&self, req: Self::Req) -> Self::Fut {
                        let request = $crate::deserialize(&req);
                        let request: __ServerSideRequest = match request {
                            ::std::result::Result::Ok(request) => request,
                            ::std::result::Result::Err(e) => {
                                return Reply::DeserializeError(deserialize_error(e));
                            }
                        };
                        match request {$(
                            __ServerSideRequest::$fn_name(( $($arg,)* )) => {
                                let reply = FutureService::$fn_name(&self.0, $($arg),*);
                                let serialize = $crate::serialize_reply
                                    as fn($crate::Result<$out>) -> $crate::SerializeFuture;
                                let reply = $crate::futures::Future::then(reply, serialize);
                                return Reply::$fn_name(reply);
                            }
                        )*}

                        #[inline]
                        fn deserialize_error(e: $crate::Error) -> $crate::SerializeFuture
                        {
                            let err: ::std::result::Result<(), _> =
                                ::std::result::Result::Err(
                                    $crate::Error::Rpc($crate::RpcError {
                                        code: $crate::RpcErrorCode::BadRequest,
                                        description:
                                            format!("Failed to deserialize request packet: {}", e)
                                    }));
                            $crate::serialize_reply(err)
                        }
                    }
                }
            }
        }

        impl<A> FutureServiceExt for A where A: FutureService {}
        impl<S> SyncServiceExt for S where S: SyncService {}


        /// Defines the blocking RPC service.
        pub trait SyncService:
            ::std::marker::Send +
            ::std::clone::Clone +
            'static
        {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> $crate::Result<$out>;
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
/// The client stub that makes RPC calls to the server. Exposes a blocking interface.
        pub struct SyncClient(FutureClient);

        impl $crate::Connect for SyncClient {
            fn connect<A>(addr: A) -> $crate::futures::BoxFuture<Self, $crate::Error>
                where A: ::std::net::ToSocketAddrs,
            {
                let client = <FutureClient as $crate::Connect>::connect(addr);
                let client = $crate::futures::Future::map(client, SyncClient);
                let client = $crate::futures::Future::map_err(client, Into::into);
                $crate::futures::Future::boxed(client)
            }
        }

        impl SyncClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*) -> $crate::Result<$out> {
                    let rpc = (self.0).$fn_name($($arg),*);
                    $crate::futures::Future::wait(rpc)
                }
            )*
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
/// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct FutureClient($crate::Client);

        impl $crate::Connect for FutureClient {
            fn connect<A>(addr: A) -> $crate::futures::BoxFuture<Self, $crate::Error>
                where A: ::std::net::ToSocketAddrs,
            {
                let client = $crate::Client::connect(addr);
                let client = $crate::futures::Future::map(client, FutureClient);
                let client = $crate::futures::Future::map_err(client, Into::into);
                $crate::futures::Future::boxed(client)
            }
        }

        impl FutureClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*)
                    -> impl $crate::futures::Future<Item=$out, Error=$crate::Error> + 'static
                {
                    future_enum! {
                        enum Fut<C, F> {
                            Called(C),
                            Failed(F)
                        }
                    }

                    let args = ($($arg,)*);
                    let req = &__ClientSideRequest::$fn_name(&args);
                    let req = match $crate::Packet::serialize(&req) {
                        Err(e) => return Fut::Failed($crate::futures::failed(e)),
                        Ok(req) => req,
                    };
                    let fut = $crate::tokio_service::Service::call(&self.0, req);
                    Fut::Called($crate::futures::Future::then(fut, move |msg| {
                        let msg: Vec<u8> = try!(msg);
                        let msg: $crate::Result<::std::result::Result<$out, $crate::RpcError>>
                            = $crate::deserialize(&msg);
                        Ok(try!(try!(msg)))
                    }))
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
    use Connect;
    use futures::{Future, failed};
    extern crate env_logger;

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
    }

    mod sync {
        use Connect;
        use futures::Future;
        use super::env_logger;
        use super::{SyncClient, SyncService, SyncServiceExt};

        #[derive(Clone, Copy)]
        struct Server;

        impl SyncService for Server {
            fn add(&self, x: i32, y: i32) -> ::Result<i32> {
                Ok(x + y)
            }
            fn hey(&self, name: String) -> ::Result<String> {
                Ok(format!("Hey, {}.", name))
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = SyncClient::connect(handle.local_addr()).wait().unwrap();
            assert_eq!(3, client.add(&1, &2).unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".to_string()).unwrap());
        }

        #[test]
        fn clone() {
            let handle = Server.listen("localhost:0").unwrap();
            let client1 = SyncClient::connect(handle.local_addr()).wait().unwrap();
            let client2 = client1.clone();
            assert_eq!(3, client1.add(&1, &2).unwrap());
            assert_eq!(3, client2.add(&1, &2).unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client =
                super::other_service::SyncClient::connect(handle.local_addr()).wait().unwrap();
            match client.foo().err().unwrap() {
                ::Error::Rpc(::RpcError { code: ::RpcErrorCode::BadRequest, .. }) => {} // good
                bad => panic!("Expected RpcError(BadRequest) but got {}", bad),
            }
        }
    }

    mod future {
        use Connect;
        use futures::{Future, finished};
        use super::env_logger;
        use super::{FutureClient, FutureService, FutureServiceExt};

        #[derive(Clone)]
        struct Server;

        impl FutureService for Server {
            fn add(&self, x: i32, y: i32) -> ::Future<i32> {
                finished(x + y).boxed()
            }

            fn hey(&self, name: String) -> ::Future<String> {
                finished(format!("Hey, {}.", name)).boxed()
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = FutureClient::connect(handle.local_addr()).wait().unwrap();
            assert_eq!(3, client.add(&1, &2).wait().unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".to_string()).wait().unwrap());
        }

        #[test]
        fn clone() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client1 = FutureClient::connect(handle.local_addr()).wait().unwrap();
            let client2 = client1.clone();
            assert_eq!(3, client1.add(&1, &2).wait().unwrap());
            assert_eq!(3, client2.add(&1, &2).wait().unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client =
                super::other_service::FutureClient::connect(handle.local_addr()).wait().unwrap();
            match client.foo().wait().err().unwrap() {
                ::Error::Rpc(::RpcError { code: ::RpcErrorCode::BadRequest, .. }) => {} // good
                bad => panic!(r#"Expected RpcError(WrongService) but got "{}""#, bad),
            }
        }
    }

    pub mod error_service {
        service! {
            rpc bar() -> u32;
        }
    }

    #[derive(Clone)]
    struct ErrorServer;

    impl error_service::FutureService for ErrorServer {
        fn bar(&self) -> ::Future<u32> {
            info!("Called bar");
            failed(::RpcError {
                        code: ::RpcErrorCode::BadRequest,
                        description: "lol jk".to_string(),
                    }
                    .into())
                .boxed()
        }
    }

    #[test]
    fn error() {
        use self::error_service::*;
        let _ = env_logger::init();

        let handle = ErrorServer.listen("localhost:0").unwrap();
        let client = FutureClient::connect(handle.local_addr()).wait().unwrap();
        client.bar()
            .then(move |result| {
                match result.err().unwrap() {
                    ::Error::Rpc(::RpcError { code: ::RpcErrorCode::BadRequest, description }) => {
                        assert_eq!(description, "lol jk");
                        Ok::<_, ()>(())
                    } // good
                    bad => panic!("Expected RpcError(BadRequest) but got {:?}", bad),
                }
            })
            .wait()
            .unwrap();

        let client = SyncClient::connect(handle.local_addr()).wait().unwrap();
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
}
