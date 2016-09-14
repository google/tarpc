// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[doc(hidden)]
#[macro_export]
macro_rules! as_item {
    ($i:item) => {$i};
}

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
        future_enum! {
            $(#[$attr:meta])*
            (pub) enum $name<$($tp),*> {
                $(#[$attrv])*
                $($variant($inner)),*
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
        future_enum! {
            $(#[$attr:meta])*
            () enum $name<$($tp),*> {
                $(#[$attrv])*
                $($variant($inner)),*
            }
        }
    };
    {
        $(#[$attr:meta])*
        ($($vis:tt)*) enum $name:ident<$($tp:ident),*> {
            $(#[$attrv:meta])*
            $($variant:ident($inner:ty)),*
        }
    } => {
        $(#[$attr])*
        as_item! {
            $($vis)* enum $name<$($tp),*> {
                $(#[$attrv])*
                $($variant($inner)),*
            }
        }

        #[allow(non_camel_case_types)]
        impl<__future_enum_T, __future_enum_E, $($tp),*> $crate::futures::Future for $name<$($tp),*>
            where __future_enum_T: Send + 'static,
                  $($inner: $crate::futures::Future<Item=__future_enum_T, Error=__future_enum_E>),*
        {
            type Item = __future_enum_T;
            type Error = __future_enum_E;

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
macro_rules! impl_serialize {
    ($impler:ident, { $($lifetime:tt)* }, $(@($name:ident $n:expr))* -- #($_n:expr) ) => {
        as_item! {
            impl$($lifetime)* $crate::serde::Serialize for $impler$($lifetime)* {
                #[inline]
                fn serialize<S>(&self, __impl_serialize_serializer: &mut S)
                    -> ::std::result::Result<(), S::Error>
                    where S: $crate::serde::Serializer
                {
                    match *self {
                        $(
                            $impler::$name(ref __impl_serialize_field) =>
                                $crate::serde::Serializer::serialize_newtype_variant(
                                    __impl_serialize_serializer,
                                    stringify!($impler),
                                    $n,
                                    stringify!($name),
                                    __impl_serialize_field,
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
            #[allow(non_camel_case_types)]
            fn deserialize<__impl_deserialize_D>(
                __impl_deserialize_deserializer: &mut __impl_deserialize_D)
                -> ::std::result::Result<$impler, __impl_deserialize_D::Error>
                where __impl_deserialize_D: $crate::serde::Deserializer
            {
                #[allow(non_camel_case_types, unused)]
                enum __impl_deserialize_Field {
                    $($name),*
                }

                impl $crate::serde::Deserialize for __impl_deserialize_Field {
                    #[inline]
                    fn deserialize<D>(__impl_deserialize_deserializer: &mut D)
                        -> ::std::result::Result<__impl_deserialize_Field, D::Error>
                        where D: $crate::serde::Deserializer
                    {
                        struct __impl_deserialize_FieldVisitor;
                        impl $crate::serde::de::Visitor for __impl_deserialize_FieldVisitor {
                            type Value = __impl_deserialize_Field;

                            #[inline]
                            fn visit_usize<E>(&mut self, __impl_deserialize_value: usize)
                                -> ::std::result::Result<__impl_deserialize_Field, E>
                                where E: $crate::serde::de::Error,
                            {
                                $(
                                    if __impl_deserialize_value == $n {
                                        return ::std::result::Result::Ok(
                                            __impl_deserialize_Field::$name);
                                    }
                                )*
                                ::std::result::Result::Err(
                                    $crate::serde::de::Error::custom(
                                        format!("No variants have a value of {}!",
                                                __impl_deserialize_value))
                                )
                            }
                        }
                        __impl_deserialize_deserializer.deserialize_struct_field(
                            __impl_deserialize_FieldVisitor)
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
                                __impl_deserialize_Field::$name => {
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
                __impl_deserialize_deserializer.deserialize_enum(
                    stringify!($impler), VARIANTS, Visitor)
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
/// # #![plugin(tarpc_plugins)]
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
        service! {
            { }

            $(
                $(#[$attr])*
                rpc $fn_name( $( $arg : $in_ ),* ) -> $out | $error;
            )*

            {
                #[allow(non_camel_case_types, unused)]
                #[derive(Debug)]
                enum __ClientSideRequest<'a> {
                    $(
                        $fn_name(&'a ( $(&'a $in_,)* ))
                    ),*
                }

                impl_serialize!(__ClientSideRequest, { <'__a> }, $($fn_name(($($in_),*)))*);
            }
        }
    };
    // Pattern for when all return types and the client request have been expanded
    (
        { } // none left to expand
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident ( $( $arg:ident : $in_:ty ),* ) -> $out:ty | $error:ty;
        )*
        {
            $client_req:item
            $client_serialize_impl:item
        }
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
                    /// The type of future returned by `{}`.
                    type $fn_name: $crate::futures::Future<Item=$out, Error=$error>;
                }

                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> ty_snake_to_camel!(Self::$fn_name);
            )*
        }

        /// Provides a function for starting the service. This is a separate trait from
        /// `FutureService` to prevent collisions with the names of RPCs.
        pub trait FutureServiceExt: FutureService {
            /// Spawns the service, binding to the given address and running on
            /// the default tokio `Loop`.
            fn listen<L>(self, addr: L) -> $crate::ListenFuture
                where L: ::std::net::ToSocketAddrs
            {
                return $crate::listen(addr, __tarpc_service_AsyncServer(self));

                #[allow(non_camel_case_types)]
                #[derive(Clone)]
                struct __tarpc_service_AsyncServer<S>(S);

                impl<S> ::std::fmt::Debug for __tarpc_service_AsyncServer<S> {
                    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(fmt, "__tarpc_service_AsyncServer {{ .. }}")
                    }
                }

                #[allow(non_camel_case_types)]
                enum __tarpc_service_Reply<__tarpc_service_S: FutureService> {
                    DeserializeError($crate::SerializeFuture),
                    $($fn_name($crate::futures::Then<
                                   $crate::futures::MapErr<
                                       ty_snake_to_camel!(__tarpc_service_S::$fn_name),
                                       fn($error) -> $crate::WireError<$error>>,
                                   $crate::SerializeFuture,
                                   fn(::std::result::Result<$out, $crate::WireError<$error>>)
                                       -> $crate::SerializeFuture>)),*
                }

                impl<S: FutureService> $crate::futures::Future for __tarpc_service_Reply<S> {
                    type Item = $crate::SerializedReply;
                    type Error = ::std::io::Error;

                    fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                        match *self {
                            __tarpc_service_Reply::DeserializeError(ref mut f) => {
                                $crate::futures::Future::poll(f)
                            }
                            $(
                                __tarpc_service_Reply::$fn_name(ref mut f) => {
                                    $crate::futures::Future::poll(f)
                                }
                            ),*
                        }
                    }
                }


                #[allow(non_camel_case_types)]
                impl<__tarpc_service_S> $crate::tokio_service::Service
                    for __tarpc_service_AsyncServer<__tarpc_service_S>
                    where __tarpc_service_S: FutureService
                {
                    type Request = ::std::vec::Vec<u8>;
                    type Response = $crate::SerializedReply;
                    type Error = ::std::io::Error;
                    type Future = __tarpc_service_Reply<__tarpc_service_S>;

                    fn poll_ready(&self) -> $crate::futures::Async<()> {
                        $crate::futures::Async::Ready(())
                    }

                    fn call(&self, __tarpc_service_req: Self::Request) -> Self::Future {
                        #[allow(non_camel_case_types, unused)]
                        #[derive(Debug)]
                        enum __tarpc_service_ServerSideRequest {
                            $(
                                $fn_name(( $($in_,)* ))
                            ),*
                        }

                        impl_deserialize!(__tarpc_service_ServerSideRequest,
                                          $($fn_name(($($in_),*)))*);

                        let __tarpc_service_request = $crate::deserialize(&__tarpc_service_req);
                        let __tarpc_service_request: __tarpc_service_ServerSideRequest =
                            match __tarpc_service_request {
                                ::std::result::Result::Ok(__tarpc_service_request) => {
                                    __tarpc_service_request
                                }
                                ::std::result::Result::Err(__tarpc_service_e) => {
                                    return __tarpc_service_Reply::DeserializeError(
                                        deserialize_error(__tarpc_service_e));
                                }
                            };
                        match __tarpc_service_request {$(
                            __tarpc_service_ServerSideRequest::$fn_name(( $($arg,)* )) => {
                                const SERIALIZE:
                                    fn(::std::result::Result<$out, $crate::WireError<$error>>)
                                    -> $crate::SerializeFuture
                                        = $crate::serialize_reply;
                                const TO_APP: fn($error) -> $crate::WireError<$error> =
                                    $crate::WireError::App;

                                return __tarpc_service_Reply::$fn_name(
                                    $crate::futures::Future::then(
                                        $crate::futures::Future::map_err(
                                            FutureService::$fn_name(&self.0, $($arg),*),
                                            TO_APP),
                                            SERIALIZE));
                            }
                        )*}

                        #[inline]
                        fn deserialize_error<E: ::std::error::Error>(__tarpc_service_e: E)
                            -> $crate::SerializeFuture
                        {
                            $crate::serialize_reply(
                                // The type param is only used in the Error::App variant, so it
                                // doesn't matter what we specify it as here.
                                ::std::result::Result::Err::<(), _>(
                                    $crate::WireError::ServerDeserialize::<$crate::util::Never>(
                                        __tarpc_service_e.to_string())))
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
            /// Spawns the service, binding to the given address and running on
            /// the default tokio `Loop`.
            fn listen<L>(self, addr: L)
                -> $crate::tokio_proto::server::ServerHandle
                where L: ::std::net::ToSocketAddrs
            {

                let __tarpc_service_service = __SyncServer {
                    service: self,
                };
                return ::std::result::Result::unwrap($crate::futures::Future::wait(FutureServiceExt::listen(__tarpc_service_service, addr)));

                #[derive(Clone)]
                struct __SyncServer<S> {
                    service: S,
                }

                #[allow(non_camel_case_types)]
                impl<__tarpc_service_S> FutureService for __SyncServer<__tarpc_service_S>
                    where __tarpc_service_S: SyncService
                {
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
                            let (__tarpc_service_complete, __tarpc_service_promise) =
                                $crate::futures::oneshot();
                            let __tarpc_service_service = self.clone();
                            const UNIMPLEMENTED: fn($crate::futures::Canceled) -> $error =
                                unimplemented;
                            ::std::thread::spawn(move || {
                                let __tarpc_service_reply = SyncService::$fn_name(
                                    &__tarpc_service_service.service, $($arg),*);
                                __tarpc_service_complete.complete(
                                    $crate::futures::IntoFuture::into_future(
                                        __tarpc_service_reply));
                            });
                            let __tarpc_service_promise =
                                $crate::futures::Future::map_err(
                                    __tarpc_service_promise, UNIMPLEMENTED);
                            $crate::futures::Future::flatten(__tarpc_service_promise)
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
                let addr = if let ::std::option::Option::Some(a) =
                    ::std::iter::Iterator::next(&mut addrs)
                {
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
                pub fn $fn_name(&self, $($arg: &$in_),*)
                    -> ::std::result::Result<$out, $crate::Error<$error>>
                {
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

        impl FutureClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                #[inline]
                pub fn $fn_name(&self, $($arg: &$in_),*)
                    -> impl $crate::futures::Future<Item=$out, Error=$crate::Error<$error>>
                    + 'static
                {
                    $client_req
                    $client_serialize_impl

                    future_enum! {
                        enum Fut<C, F> {
                            Called(C),
                            Failed(F)
                        }
                    }

                    let __tarpc_service_args = ($($arg,)*);
                    let __tarpc_service_req = &__ClientSideRequest::$fn_name(&__tarpc_service_args);
                    let __tarpc_service_req =
                        match $crate::Packet::serialize(&__tarpc_service_req)
                    {
                        ::std::result::Result::Err(__tarpc_service_e) => {
                            return Fut::Failed(
                                $crate::futures::failed(
                                    $crate::Error::ClientSerialize(__tarpc_service_e)))
                        }
                        ::std::result::Result::Ok(__tarpc_service_req) => __tarpc_service_req,
                    };
                    let __tarpc_service_fut =
                        $crate::tokio_service::Service::call(&self.0, __tarpc_service_req);
                    Fut::Called($crate::futures::Future::then(__tarpc_service_fut,
                                                              move |__tarpc_service_msg| {
                        let __tarpc_service_msg: Vec<u8> = try!(__tarpc_service_msg);
                        let __tarpc_service_msg:
                            ::std::result::Result<
                            ::std::result::Result<$out, $crate::WireError<$error>>, _>
                            = $crate::deserialize(&__tarpc_service_msg);
                        match __tarpc_service_msg {
                            ::std::result::Result::Ok(__tarpc_service_msg) => {
                                ::std::result::Result::Ok(try!(__tarpc_service_msg))
                            }
                            ::std::result::Result::Err(__tarpc_service_e) => {
                                ::std::result::Result::Err(
                                    $crate::Error::ClientDeserialize(__tarpc_service_e))
                            }
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
        #[deny(warnings)]
        #[allow(non_snake_case)]
        rpc TestCamelCaseDoesntConflict();
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
        use super::{SyncClient, SyncService, SyncServiceExt};
        use super::env_logger;
        use sync::Connect;
        use util::Never;

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
            let handle = Server.listen("localhost:0");
            let client = SyncClient::connect(handle.local_addr()).unwrap();
            assert_eq!(3, client.add(&1, &2).unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".to_string()).unwrap());
        }

        #[test]
        fn clone() {
            let handle = Server.listen("localhost:0");
            let client1 = SyncClient::connect(handle.local_addr()).unwrap();
            let client2 = client1.clone();
            assert_eq!(3, client1.add(&1, &2).unwrap());
            assert_eq!(3, client2.add(&1, &2).unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0");
            let client = super::other_service::SyncClient::connect(handle.local_addr()).unwrap();
            match client.foo().err().unwrap() {
                ::Error::ServerDeserialize(_) => {} // good
                bad => panic!("Expected Error::ServerDeserialize but got {}", bad),
            }
        }
    }

    mod future {
        use future::Connect;
        use futures::{Finished, Future, finished};
        use super::{FutureClient, FutureService, FutureServiceExt};
        use super::env_logger;
        use util::Never;

        #[derive(Clone)]
        struct Server;

        impl FutureService for Server {
            type AddFut = Finished<i32, Never>;

            fn add(&self, x: i32, y: i32) -> Self::AddFut {
                finished(x + y)
            }

            type HeyFut = Finished<String, Never>;

            fn hey(&self, name: String) -> Self::HeyFut {
                finished(format!("Hey, {}.", name))
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").wait().unwrap();
            let client = FutureClient::connect(handle.local_addr()).wait().unwrap();
            assert_eq!(3, client.add(&1, &2).wait().unwrap());
            assert_eq!("Hey, Tim.", client.hey(&"Tim".to_string()).wait().unwrap());
        }

        #[test]
        fn clone() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").wait().unwrap();
            let client1 = FutureClient::connect(handle.local_addr()).wait().unwrap();
            let client2 = client1.clone();
            assert_eq!(3, client1.add(&1, &2).wait().unwrap());
            assert_eq!(3, client2.add(&1, &2).wait().unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::init();
            let handle = Server.listen("localhost:0").wait().unwrap();
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
        type BarFut = ::futures::Failed<u32, ::util::Message>;

        fn bar(&self) -> Self::BarFut {
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

        let handle = ErrorServer.listen("localhost:0").wait().unwrap();
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
