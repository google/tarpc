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
        pub enum $name:ident<$($tp:ident),*> {
            $(#[$attrv:meta])*
            $($variant:ident($inner:ty)),*
        }
    } => {
        $(#[$attr])*
        pub enum $name<$($tp),*> {
            $(#[$attrv])*
            $($variant($inner)),*
        }

        impl<__T, __E, $($tp),*> $crate::futures::Future for $name<$($tp),*>
            where __T: ::std::marker::Send + 'static,
                  $($inner: $crate::futures::Future<Item=__T, Error=__E>),*
        {
            type Item = __T;
            type Error = __E;

            fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                match *self {
                    $($name::$variant(ref mut f) => $crate::futures::Future::poll(f)),*
                }
            }
        }
    };
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

        impl<__T, __E, $($tp),*> $crate::futures::Future for $name<$($tp),*>
            where __T: ::std::marker::Send + 'static,
                  $($inner: $crate::futures::Future<Item=__T, Error=__E>),*
        {
            type Item = __T;
            type Error = __E;

            fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
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
/// # #![feature(conservative_impl_trait, plugin)]
/// # #![plugin(snake_to_camel)]
/// # #[macro_use] extern crate tarpc;
/// # fn main() {}
/// # service! {
/// /// Say hello
/// rpc hello(name: String) -> String;
/// # }
/// ```
///
/// Attributes can be attached to each rpc. These attributes
/// will then be attached to the generated service traits'
/// corresponding `fn`s, as well as to the client stubs' RPCs.
///
/// The following items are expanded in the enclosing module:
///
/// * `FutureService` -- the trait defining the RPC service via a `Future` API.
/// * `SyncService` -- a service trait that provides a synchronous API for when
///                    spawning a thread per request is acceptable.
/// * `FutureServiceExt` -- provides the methods for starting a service. There is an umbrella impl
///                         for all implers of `FutureService`. It's a separate trait to prevent
///                         name collisions with RPCs.
/// * `SyncServiceExt` -- same as `FutureServiceExt` but for `SyncService`.
/// * `FutureClient` -- a client whose RPCs return `Future`s.
/// * `SyncClient` -- a client whose RPCs block until the reply is available. Easiest
///                   interface to use, as it looks the same as a regular function call.
///
#[macro_export]
macro_rules! service {
// Entry point
    (
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) $(-> $out:ty)* $(| $error:ty)*;
        )*
    ) => {
        service! {{
            $(
                $(#[$attr])*
                rpc $fn_name( $( $arg : $in_ ),* ) $(-> $out)* $(| $error)*;
            )*
        }}
    };
// Pattern for when the next rpc has an implicit unit return type and no error type.
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
            rpc $fn_name( $( $arg : $in_ ),* ) -> () | $crate::util::Never;
        }
    };
// Pattern for when the next rpc has an explicit return type and no error type.
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
            rpc $fn_name( $( $arg : $in_ ),* ) -> $out | $crate::util::Never;
        }
    };
// Pattern for when the next rpc has an implicit unit return type and an explicit error type.
    (
        {
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) | $error:ty;

            $( $unexpanded:tt )*
        }
        $( $expanded:tt )*
    ) => {
        service! {
            { $( $unexpanded )* }

            $( $expanded )*

            $(#[$attr])*
            rpc $fn_name( $( $arg : $in_ ),* ) -> () | $error;
        }
    };
// Pattern for when the next rpc has an explicit return type and an explicit error type.
    (
        {
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty | $error:ty;

            $( $unexpanded:tt )*
        }
        $( $expanded:tt )*
    ) => {
        service! {
            { $( $unexpanded )* }

            $( $expanded )*

            $(#[$attr])*
            rpc $fn_name( $( $arg : $in_ ),* ) -> $out | $error;
        }
    };
// Pattern for when all return types have been expanded
    (
        { } // none left to expand
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident ( $( $arg:ident : $in_:ty ),* ) -> $out:ty | $error:ty;
        )*
    ) => {

/// Defines the `Future` RPC service. Implementors must be `Clone`, `Send`, and `'static`,
/// as required by `tokio_proto::NewService`. This is required so that the service can be used
/// to respond to multiple requests concurrently.
        pub trait FutureService:
            ::std::marker::Send +
            ::std::clone::Clone +
            'static
        {
            $(

                snake_to_camel! {
                    /// The type of future returned by the fn of the same name.
                    type $fn_name: $crate::futures::Future<Item=$out, Error=$error>;
                }

                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> ty_snake_to_camel!(Self::$fn_name);
            )*
        }

        /// Provides a function for starting the service. This is a separate trait from
        /// `FutureService` to prevent collisions with the names of RPCs.
        pub trait FutureServiceExt: FutureService {
            /// Registers the service with the given registry, listening on the given address.
            fn listen<L>(self, addr: L)
                -> ::std::io::Result<$crate::tokio_proto::server::ServerHandle>
                where L: ::std::net::ToSocketAddrs
            {
                return $crate::listen(addr, __AsyncServer(self));

                #[derive(Clone)]
                struct __AsyncServer<S>(S);

                impl<S> ::std::fmt::Debug for __AsyncServer<S> {
                    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(fmt, "__AsyncServer {{ .. }}")
                    }
                }

                #[allow(non_camel_case_types)]
                enum Reply<TyParamS: FutureService> {
                    DeserializeError($crate::SerializeFuture),
                    $($fn_name($crate::futures::Then<$crate::futures::MapErr<ty_snake_to_camel!(TyParamS::$fn_name),
                                                                             fn($error) -> $crate::WireError<$error>>,
                                                     $crate::SerializeFuture,
                                                     fn(::std::result::Result<$out, $crate::WireError<$error>>)
                                                         -> $crate::SerializeFuture>)),*
                }

                impl<S: FutureService> $crate::futures::Future for Reply<S> {
                    type Item = $crate::SerializedReply;
                    type Error = ::std::io::Error;

                    fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
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
                    type Error = ::std::io::Error;
                    type Fut = Reply<S>;

                    fn call(&self, req: Self::Req) -> Self::Fut {
                        #[allow(non_camel_case_types, unused)]
                        #[derive(Debug)]
                        enum __ServerSideRequest {
                            $(
                                $fn_name(( $($in_,)* ))
                            ),*
                        }

                        impl_deserialize!(__ServerSideRequest, $($fn_name(($($in_),*)))*);

                        let request = $crate::deserialize(&req);
                        let request: __ServerSideRequest = match request {
                            ::std::result::Result::Ok(request) => request,
                            ::std::result::Result::Err(e) => {
                                return Reply::DeserializeError(deserialize_error(e));
                            }
                        };
                        match request {$(
                            __ServerSideRequest::$fn_name(( $($arg,)* )) => {
                                const SERIALIZE: fn(::std::result::Result<$out, $crate::WireError<$error>>)
                                    -> $crate::SerializeFuture = $crate::serialize_reply;
                                const TO_APP: fn($error) -> $crate::WireError<$error> = $crate::WireError::App;

                                let reply = FutureService::$fn_name(&self.0, $($arg),*);
                                let reply = $crate::futures::Future::map_err(reply, TO_APP);
                                let reply = $crate::futures::Future::then(reply, SERIALIZE);
                                return Reply::$fn_name(reply);
                            }
                        )*}

                        #[inline]
                        fn deserialize_error<E: ::std::error::Error>(e: E) -> $crate::SerializeFuture {
                            let err = $crate::WireError::ServerDeserialize::<$crate::util::Never>(e.to_string());
                            $crate::serialize_reply(::std::result::Result::Err::<(), _>(err))
                        }
                    }
                }
            }
        }

/// Defines the blocking RPC service. Must be `Clone`, `Send`, and `'static`,
/// as required by `tokio_proto::NewService`. This is required so that the service can be used
/// to respond to multiple requests concurrently.
        pub trait SyncService:
            ::std::marker::Send +
            ::std::clone::Clone +
            'static
        {
            $(
                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> ::std::result::Result<$out, $error>;
            )*
        }

        /// Provides a function for starting the service. This is a separate trait from
        /// `SyncService` to prevent collisions with the names of RPCs.
        pub trait SyncServiceExt: SyncService {
            /// Registers the service with the given registry, listening on the given address.
            fn listen<L>(self, addr: L)
                -> ::std::io::Result<$crate::tokio_proto::server::ServerHandle>
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
                        impl_snake_to_camel! {
                            type $fn_name =
                                $crate::futures::Flatten<
                                    $crate::futures::MapErr<
                                        $crate::futures::Oneshot<
                                            $crate::futures::Done<$out, $error>>,
                                        fn($crate::futures::Canceled) -> $error>>;
                        }
                        fn $fn_name(&self, $($arg:$in_),*) -> ty_snake_to_camel!(Self::$fn_name) {
                            fn unimplemented(_: $crate::futures::Canceled) -> $error {
                                // TODO(tikue): what do do if SyncService panics?
                                unimplemented!()
                            }
                            let (c, p) = $crate::futures::oneshot();
                            let service = self.clone();
                            ::std::thread::spawn(move || {
                                let reply = SyncService::$fn_name(&service.service, $($arg),*);
                                c.complete($crate::futures::IntoFuture::into_future(reply));
                            });
                            let p = $crate::futures::Future::map_err(p, unimplemented as fn($crate::futures::Canceled) -> $error);
                            $crate::futures::Future::flatten(p)
                        }
                    )*
                }
            }
        }

        impl<A> FutureServiceExt for A where A: FutureService {}
        impl<S> SyncServiceExt for S where S: SyncService {}

        #[allow(unused)]
        #[derive(Clone, Debug)]
/// The client stub that makes RPC calls to the server. Exposes a blocking interface.
        pub struct SyncClient(FutureClient);

        impl $crate::sync::Connect for SyncClient {
            fn connect<A>(addr: A) -> ::std::result::Result<Self, ::std::io::Error>
                where A: ::std::net::ToSocketAddrs,
            {
                let mut addrs = try!(::std::net::ToSocketAddrs::to_socket_addrs(&addr));
                let addr = if let ::std::option::Option::Some(a) = ::std::iter::Iterator::next(&mut addrs) {
                    a
                } else {
                    return ::std::result::Result::Err(
                        ::std::io::Error::new(
                            ::std::io::ErrorKind::AddrNotAvailable,
                            "`ToSocketAddrs::to_socket_addrs` returned an empty iterator."));
                };
                let client = <FutureClient as $crate::future::Connect>::connect(&addr);
                let client = $crate::futures::Future::wait(client);
                let client = SyncClient(try!(client));
                ::std::result::Result::Ok(client)
            }
        }

        impl SyncClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*) -> ::std::result::Result<$out, $crate::Error<$error>> {
                    let rpc = (self.0).$fn_name($($arg),*);
                    $crate::futures::Future::wait(rpc)
                }
            )*
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
/// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct FutureClient($crate::Client);

        impl $crate::future::Connect for FutureClient {
            type Fut = $crate::futures::Map<$crate::ClientFuture, fn($crate::Client) -> Self>;

            fn connect(addr: &::std::net::SocketAddr) -> Self::Fut {
                let client = <$crate::Client as $crate::future::Connect>::connect(addr);
                $crate::futures::Future::map(client, FutureClient)
            }
        }

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum __ClientSideRequest<'a> {
            $(
                $fn_name(&'a ( $(&'a $in_,)* ))
            ),*
        }

        impl_serialize!(__ClientSideRequest, { <'__a> }, $($fn_name(($($in_),*)))*);

        impl FutureClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*)
                    -> impl $crate::futures::Future<Item=$out, Error=$crate::Error<$error>> + 'static
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
                        ::std::result::Result::Err(e) => return Fut::Failed($crate::futures::failed($crate::Error::ClientSerialize(e))),
                        ::std::result::Result::Ok(req) => req,
                    };
                    let fut = $crate::tokio_service::Service::call(&self.0, req);
                    Fut::Called($crate::futures::Future::then(fut, move |msg| {
                        let msg: Vec<u8> = try!(msg);
                        let msg: ::std::result::Result<::std::result::Result<$out, $crate::WireError<$error>>, _>
                            = $crate::deserialize(&msg);
                        match msg {
                            ::std::result::Result::Ok(msg) => ::std::result::Result::Ok(try!(msg)),
                            ::std::result::Result::Err(e) => ::std::result::Result::Err($crate::Error::ClientDeserialize(e)),
                        }
                    }))
                }
            )*

        }
    }
}

// allow dead code; we're just testing that the macro expansion compiles
#[allow(dead_code)]
#[cfg(test)]
mod syntax_test {
    use util::Never;
    service! {
        rpc hello() -> String;
        #[doc="attr"]
        rpc attr(s: String) -> String;
        rpc no_args_no_return();
        rpc no_args() -> ();
        rpc one_arg(foo: String) -> i32;
        rpc two_args_no_return(bar: String, baz: u64);
        rpc two_args(bar: String, baz: u64) -> String;
        rpc no_args_ret_error() -> i32 | Never;
        rpc one_arg_ret_error(foo: String) -> String | Never;
        rpc no_arg_implicit_return_error() | Never;
        #[doc="attr"]
        rpc one_arg_implicit_return_error(foo: String) | Never;
    }
}

#[cfg(test)]
mod functional_test {
    use futures::{Future, failed};
    extern crate env_logger;

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
    }

    mod sync {
        use sync::Connect;
        use util::Never;
        use super::env_logger;
        use super::{SyncClient, SyncService, SyncServiceExt};

        #[derive(Clone, Copy)]
        struct Server;

        impl SyncService for Server {
            fn add(&self, x: i32, y: i32) -> Result<i32, Never> {
                Ok(x + y)
            }
            fn hey(&self, name: String) -> Result<String, Never> {
                Ok(format!("Hey, {}.", name))
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = SyncClient::connect(handle.local_addr()).unwrap();
            assert_eq!(3, client.add(&1, &2).unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".to_string()).unwrap());
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
        fn other_service() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").unwrap();
            let client = super::other_service::SyncClient::connect(handle.local_addr()).unwrap();
            match client.foo().err().unwrap() {
                ::Error::ServerDeserialize(_) => {} // good
                bad => panic!("Expected Error::ServerDeserialize but got {}", bad),
            }
        }
    }

    mod future {
        use future::Connect;
        use util::Never;
        use futures::{Finished, Future, finished};
        use super::env_logger;
        use super::{FutureClient, FutureService, FutureServiceExt};

        #[derive(Clone)]
        struct Server;

        impl FutureService for Server {
            type Add = Finished<i32, Never>;

            fn add(&self, x: i32, y: i32) -> Self::Add {
                finished(x + y)
            }

            type Hey = Finished<String, Never>;

            fn hey(&self, name: String) -> Self::Hey {
                finished(format!("Hey, {}.", name))
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
                ::Error::ServerDeserialize(_) => {} // good
                bad => panic!(r#"Expected Error::ServerDeserialize but got "{}""#, bad),
            }
        }
    }

    pub mod error_service {
        service! {
            rpc bar() -> u32 | ::util::Message;
        }
    }

    #[derive(Clone)]
    struct ErrorServer;

    impl error_service::FutureService for ErrorServer {
        type Bar = ::futures::Failed<u32, ::util::Message>;

        fn bar(&self) -> Self::Bar {
            info!("Called bar");
            failed("lol jk".into())
        }
    }

    #[test]
    fn error() {
        use future::Connect as Fc;
        use sync::Connect as Sc;
        use std::error::Error as E;
        use self::error_service::*;
        let _ = env_logger::init();

        let handle = ErrorServer.listen("localhost:0").unwrap();
        let client = FutureClient::connect(handle.local_addr()).wait().unwrap();
        client.bar()
            .then(move |result| {
                match result.err().unwrap() {
                    ::Error::App(e) => {
                        assert_eq!(e.description(), "lol jk");
                        Ok::<_, ()>(())
                    } // good
                    bad => panic!("Expected Error::App but got {:?}", bad),
                }
            })
            .wait()
            .unwrap();

        let client = SyncClient::connect(handle.local_addr()).unwrap();
        match client.bar().err().unwrap() {
            ::Error::App(e) => {
                assert_eq!(e.description(), "lol jk");
            } // good
            bad => panic!("Expected Error::App but got {:?}", bad),
        }
    }

    pub mod other_service {
        service! {
            rpc foo();
        }
    }
}
