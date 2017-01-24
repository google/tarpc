// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[doc(hidden)]
#[macro_export]
macro_rules! as_item {
    ($i:item) => {$i};
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_serialize {
    ($impler:ident, { $($lifetime:tt)* }, $(@($name:ident $n:expr))* -- #($n_:expr) ) => {
        as_item! {
            impl$($lifetime)* $crate::serde::Serialize for $impler$($lifetime)* {
                fn serialize<S>(&self, impl_serialize_serializer__: &mut S)
                    -> ::std::result::Result<(), S::Error>
                    where S: $crate::serde::Serializer
                {
                    match *self {
                        $(
                            $impler::$name(ref impl_serialize_field__) =>
                                $crate::serde::Serializer::serialize_newtype_variant(
                                    impl_serialize_serializer__,
                                    stringify!($impler),
                                    $n,
                                    stringify!($name),
                                    impl_serialize_field__,
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
    ($impler:ident, $(@($name:ident $n:expr))* -- #($n_:expr) ) => (
        impl $crate::serde::Deserialize for $impler {
            #[allow(non_camel_case_types)]
            fn deserialize<impl_deserialize_D__>(
                impl_deserialize_deserializer__: &mut impl_deserialize_D__)
                -> ::std::result::Result<$impler, impl_deserialize_D__::Error>
                where impl_deserialize_D__: $crate::serde::Deserializer
            {
                #[allow(non_camel_case_types, unused)]
                enum impl_deserialize_Field__ {
                    $($name),*
                }

                impl $crate::serde::Deserialize for impl_deserialize_Field__ {
                    fn deserialize<D>(impl_deserialize_deserializer__: &mut D)
                        -> ::std::result::Result<impl_deserialize_Field__, D::Error>
                        where D: $crate::serde::Deserializer
                    {
                        struct impl_deserialize_FieldVisitor__;
                        impl $crate::serde::de::Visitor for impl_deserialize_FieldVisitor__ {
                            type Value = impl_deserialize_Field__;

                            fn visit_usize<E>(&mut self, impl_deserialize_value__: usize)
                                -> ::std::result::Result<impl_deserialize_Field__, E>
                                where E: $crate::serde::de::Error,
                            {
                                $(
                                    if impl_deserialize_value__ == $n {
                                        return ::std::result::Result::Ok(
                                            impl_deserialize_Field__::$name);
                                    }
                                )*
                                ::std::result::Result::Err(
                                    $crate::serde::de::Error::custom(
                                        format!("No variants have a value of {}!",
                                                impl_deserialize_value__))
                                )
                            }
                        }
                        impl_deserialize_deserializer__.deserialize_struct_field(
                            impl_deserialize_FieldVisitor__)
                    }
                }

                struct Visitor;
                impl $crate::serde::de::EnumVisitor for Visitor {
                    type Value = $impler;

                    fn visit<V>(&mut self, mut tarpc_enum_visitor__: V)
                        -> ::std::result::Result<$impler, V::Error>
                        where V: $crate::serde::de::VariantVisitor
                    {
                        match tarpc_enum_visitor__.visit_variant()? {
                            $(
                                impl_deserialize_Field__::$name => {
                                    ::std::result::Result::Ok(
                                        $impler::$name(tarpc_enum_visitor__.visit_newtype()?))
                                }
                            ),*
                        }
                    }
                }
                const TARPC_VARIANTS__: &'static [&'static str] = &[
                    $(
                        stringify!($name)
                    ),*
                ];
                impl_deserialize_deserializer__.deserialize_enum(
                    stringify!($impler), TARPC_VARIANTS__, Visitor)
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

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum tarpc_service_Request__ {
            NotIrrefutable(()),
            $(
                $fn_name(( $($in_,)* ))
            ),*
        }

        impl_deserialize!(tarpc_service_Request__, NotIrrefutable(()) $($fn_name(($($in_),*)))*);
        impl_serialize!(tarpc_service_Request__, {}, NotIrrefutable(()) $($fn_name(($($in_),*)))*);

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum tarpc_service_Response__ {
            NotIrrefutable(()),
            $(
                $fn_name($out)
            ),*
        }

        impl_deserialize!(tarpc_service_Response__, NotIrrefutable(()) $($fn_name($out))*);
        impl_serialize!(tarpc_service_Response__, {}, NotIrrefutable(()) $($fn_name($out))*);

        #[allow(non_camel_case_types, unused)]
        #[derive(Debug)]
        enum tarpc_service_Error__ {
            NotIrrefutable(()),
            $(
                $fn_name($error)
            ),*
        }

        impl_deserialize!(tarpc_service_Error__, NotIrrefutable(()) $($fn_name($error))*);
        impl_serialize!(tarpc_service_Error__, {}, NotIrrefutable(()) $($fn_name($error))*);

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
            fn listen(self,
                      addr: ::std::net::SocketAddr,
                      options: $crate::server::Options)
                -> $crate::server::ListenFuture
            {
                return $crate::server::listen(
                                           move || Ok(tarpc_service_AsyncServer__(self.clone())),
                                           addr,
                                           options);

                #[allow(non_camel_case_types)]
                #[derive(Clone)]
                struct tarpc_service_AsyncServer__<S>(S);

                impl<S> ::std::fmt::Debug for tarpc_service_AsyncServer__<S> {
                    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(fmt, "tarpc_service_AsyncServer__ {{ .. }}")
                    }
                }

                #[allow(non_camel_case_types)]
                type tarpc_service_Future__ =
                    $crate::futures::Finished<$crate::server::Response<tarpc_service_Response__,
                                                               tarpc_service_Error__>,
                                              ::std::io::Error>;

                #[allow(non_camel_case_types)]
                enum tarpc_service_FutureReply__<tarpc_service_S__: FutureService> {
                    DeserializeError(tarpc_service_Future__),
                    $($fn_name(
                            $crate::futures::Then<ty_snake_to_camel!(tarpc_service_S__::$fn_name),
                                                  tarpc_service_Future__,
                                                  fn(::std::result::Result<$out, $error>)
                                                      -> tarpc_service_Future__>)),*
                }

                impl<S: FutureService> $crate::futures::Future for tarpc_service_FutureReply__<S> {
                    type Item = $crate::server::Response<tarpc_service_Response__,
                                                         tarpc_service_Error__>;

                    type Error = ::std::io::Error;

                    fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                        match *self {
                            tarpc_service_FutureReply__::DeserializeError(
                                ref mut tarpc_service_future__) =>
                            {
                                $crate::futures::Future::poll(tarpc_service_future__)
                            }
                            $(
                                tarpc_service_FutureReply__::$fn_name(
                                    ref mut tarpc_service_future__) =>
                                {
                                    $crate::futures::Future::poll(tarpc_service_future__)
                                }
                            ),*
                        }
                    }
                }


                #[allow(non_camel_case_types)]
                impl<tarpc_service_S__> $crate::tokio_service::Service
                    for tarpc_service_AsyncServer__<tarpc_service_S__>
                    where tarpc_service_S__: FutureService
                {
                    type Request = ::std::result::Result<tarpc_service_Request__,
                                                         $crate::bincode::serde::DeserializeError>;
                    type Response = $crate::server::Response<tarpc_service_Response__,
                                                     tarpc_service_Error__>;
                    type Error = ::std::io::Error;
                    type Future = tarpc_service_FutureReply__<tarpc_service_S__>;

                    fn call(&self, tarpc_service_request__: Self::Request) -> Self::Future {
                        let tarpc_service_request__ = match tarpc_service_request__ {
                            Ok(tarpc_service_request__) => tarpc_service_request__,
                            Err(tarpc_service_deserialize_err__) => {
                                return tarpc_service_FutureReply__::DeserializeError(
                                    $crate::futures::finished(
                                        ::std::result::Result::Err(
                                            $crate::WireError::ServerDeserialize(
                                                ::std::string::ToString::to_string(
                                                    &tarpc_service_deserialize_err__)))));
                            }
                        };
                        match tarpc_service_request__ {
                            tarpc_service_Request__::NotIrrefutable(()) => unreachable!(),
                            $(
                                tarpc_service_Request__::$fn_name(( $($arg,)* )) => {
                                    fn tarpc_service_wrap__(
                                        tarpc_service_response__:
                                            ::std::result::Result<$out, $error>)
                                        -> tarpc_service_Future__
                                    {
                                        $crate::futures::finished(
                                            tarpc_service_response__
                                                .map(tarpc_service_Response__::$fn_name)
                                                .map_err(|tarpc_service_error__| {
                                                    $crate::WireError::App(
                                                        tarpc_service_Error__::$fn_name(
                                                            tarpc_service_error__))
                                                })
                                        )
                                    }
                                    return tarpc_service_FutureReply__::$fn_name(
                                        $crate::futures::Future::then(
                                                FutureService::$fn_name(&self.0, $($arg),*),
                                                tarpc_service_wrap__));
                                }
                            )*
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
            fn listen<L>(self, addr: L, options: $crate::server::Options)
                -> ::std::io::Result<::std::net::SocketAddr>
                where L: ::std::net::ToSocketAddrs
            {
                let tarpc_service__ = SyncServer__ {
                    service: self,
                };

                let tarpc_service_addr__ =
                    $crate::util::FirstSocketAddr::try_first_socket_addr(&addr)?;

                // Wrapped in a lazy future to ensure execution occurs when a task is present.
                return $crate::futures::Future::wait($crate::futures::future::lazy(move || {
                    FutureServiceExt::listen(tarpc_service__, tarpc_service_addr__, options)
                }));

                #[derive(Clone)]
                struct SyncServer__<S> {
                    service: S,
                }

                #[allow(non_camel_case_types)]
                impl<tarpc_service_S__> FutureService for SyncServer__<tarpc_service_S__>
                    where tarpc_service_S__: SyncService
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
                            let (tarpc_service_complete__, tarpc_service_promise__) =
                                $crate::futures::oneshot();
                            let tarpc_service__ = self.clone();
                            const UNIMPLEMENTED: fn($crate::futures::Canceled) -> $error =
                                unimplemented;
                            ::std::thread::spawn(move || {
                                let tarpc_service_reply__ = SyncService::$fn_name(
                                    &tarpc_service__.service, $($arg),*);
                                tarpc_service_complete__.complete(
                                    $crate::futures::IntoFuture::into_future(
                                        tarpc_service_reply__));
                            });
                            let tarpc_service_promise__ =
                                $crate::futures::Future::map_err(
                                    tarpc_service_promise__, UNIMPLEMENTED);
                            $crate::futures::Future::flatten(tarpc_service_promise__)
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

        impl $crate::client::sync::Connect for SyncClient {
            fn connect<A>(addr_: A, opts_: $crate::client::Options)
                -> ::std::result::Result<Self, ::std::io::Error>
                where A: ::std::net::ToSocketAddrs,
            {
                let addr_ = $crate::util::FirstSocketAddr::try_first_socket_addr(&addr_)?;
                // Wrapped in a lazy future to ensure execution occurs when a task is present.
                let client_ = $crate::futures::Future::wait($crate::futures::future::lazy(move || {
                    <FutureClient as $crate::client::future::Connect>::connect(addr_, opts_)
                }))?;
                let client_ = SyncClient(client_);
                ::std::result::Result::Ok(client_)
            }
        }

        impl SyncClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                pub fn $fn_name(&self, $($arg: $in_),*)
                    -> ::std::result::Result<$out, $crate::Error<$error>>
                {
                    // Wrapped in a lazy future to ensure execution occurs when a task is present.
                    $crate::futures::Future::wait($crate::futures::future::lazy(move || {
                        (self.0).$fn_name($($arg),*)
                    }))
                }
            )*
        }

        #[allow(non_camel_case_types)]
        type tarpc_service_Client__ =
            $crate::client::Client<tarpc_service_Request__,
                           tarpc_service_Response__,
                           tarpc_service_Error__>;

        #[allow(non_camel_case_types)]
        /// Implementation detail: Pending connection.
        pub struct tarpc_service_ConnectFuture__<T> {
            inner: $crate::futures::Map<$crate::client::future::ConnectFuture<
                                            tarpc_service_Request__,
                                            tarpc_service_Response__,
                                            tarpc_service_Error__>,
                                        fn(tarpc_service_Client__) -> T>,
        }

        impl<T> $crate::futures::Future for tarpc_service_ConnectFuture__<T> {
            type Item = T;
            type Error = ::std::io::Error;

            fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                $crate::futures::Future::poll(&mut self.inner)
            }
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct FutureClient(tarpc_service_Client__);

        impl<'a> $crate::client::future::Connect for FutureClient {
            type ConnectFut = tarpc_service_ConnectFuture__<Self>;

            fn connect(tarpc_service_addr__: ::std::net::SocketAddr,
                                tarpc_service_options__: $crate::client::Options)
                -> Self::ConnectFut
            {
                let client = <tarpc_service_Client__ as $crate::client::future::Connect>::connect(
                    tarpc_service_addr__, tarpc_service_options__);

                tarpc_service_ConnectFuture__ {
                    inner: $crate::futures::Future::map(client, FutureClient)
                }
            }
        }

        impl FutureClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                pub fn $fn_name(&self, $($arg: $in_),*)
                    -> impl $crate::futures::Future<Item=$out, Error=$crate::Error<$error>>
                    + 'static
                {
                    let tarpc_service_req__ = tarpc_service_Request__::$fn_name(($($arg,)*));
                    let tarpc_service_fut__ =
                        $crate::tokio_service::Service::call(&self.0, tarpc_service_req__);
                    $crate::futures::Future::then(tarpc_service_fut__,
                                                  move |tarpc_service_msg__| {
                        match tarpc_service_msg__? {
                            ::std::result::Result::Ok(tarpc_service_msg__) => {
                                if let tarpc_service_Response__::$fn_name(tarpc_service_msg__) =
                                    tarpc_service_msg__
                                {
                                    ::std::result::Result::Ok(tarpc_service_msg__)
                                } else {
                                   unreachable!()
                                }
                            }
                            ::std::result::Result::Err(tarpc_service_err__) => {
                                ::std::result::Result::Err(match tarpc_service_err__ {
                                    $crate::Error::App(tarpc_service_err__) => {
                                        if let tarpc_service_Error__::$fn_name(
                                            tarpc_service_err__) = tarpc_service_err__
                                        {
                                            $crate::Error::App(tarpc_service_err__)
                                        } else {
                                            unreachable!()
                                        }
                                    }
                                    $crate::Error::ServerDeserialize(tarpc_service_err__) => {
                                        $crate::Error::ServerDeserialize(tarpc_service_err__)
                                    }
                                    $crate::Error::ServerSerialize(tarpc_service_err__) => {
                                        $crate::Error::ServerSerialize(tarpc_service_err__)
                                    }
                                    $crate::Error::ClientDeserialize(tarpc_service_err__) => {
                                        $crate::Error::ClientDeserialize(tarpc_service_err__)
                                    }
                                    $crate::Error::ClientSerialize(tarpc_service_err__) => {
                                        $crate::Error::ClientSerialize(tarpc_service_err__)
                                    }
                                    $crate::Error::Io(tarpc_service_error__) => {
                                        $crate::Error::Io(tarpc_service_error__)
                                    }
                                })
                            }
                        }
                    })
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
    use {client, server};
    use futures::{Future, failed};
    use std::io;
    use std::net::SocketAddr;
    use util::FirstSocketAddr;
    extern crate env_logger;

    macro_rules! unwrap {
        ($e:expr) => (match $e {
            Ok(e) => e,
            Err(e) => panic!("{} failed with {:?}", stringify!($e), e),
        })
    }

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
    }

    cfg_if! {
        if #[cfg(feature = "tls")] {
            const DOMAIN: &'static str = "foobar.com";

            use client::tls::Context;
            use ::native_tls::{Pkcs12, TlsAcceptor, TlsConnector};

            fn tls_context() -> (server::Options, client::Options) {
                let buf = include_bytes!("../test/identity.p12");
                let pkcs12 = unwrap!(Pkcs12::from_der(buf, "mypass"));
                let acceptor = unwrap!(unwrap!(TlsAcceptor::builder(pkcs12)).build());
                let server_options = server::Options::default().tls(acceptor);
                let client_options = get_tls_client_options();

                (server_options, client_options)
            }

            // Making the TlsConnector for testing needs to be OS-dependent just like native-tls.
            // We need to go through this trickery because the test self-signed cert is not part
            // of the system's cert chain. If it was, then all that is required is
            // `TlsConnector::builder().unwrap().build().unwrap()`.
            cfg_if! {
                if #[cfg(target_os = "macos")] {
                    extern crate security_framework;

                    use self::security_framework::certificate::SecCertificate;
                    use native_tls::backend::security_framework::TlsConnectorBuilderExt;

                    fn get_tls_client_options() -> client::Options {
                        let buf = include_bytes!("../test/root-ca.der");
                        let cert = unwrap!(SecCertificate::from_der(buf));
                        let mut connector = unwrap!(TlsConnector::builder());
                        connector.anchor_certificates(&[cert]);

                        client::Options::default().tls(Context {
                            domain: DOMAIN.into(),
                            tls_connector: unwrap!(connector.build()),
                        })
                    }
                } else if #[cfg(all(not(target_os = "macos"), not(windows)))] {
                    use native_tls::backend::openssl::TlsConnectorBuilderExt;

                    fn get_tls_client_options() -> client::Options {
                        let mut connector = unwrap!(TlsConnector::builder());
                        unwrap!(connector.builder_mut()
                           .builder_mut()
                           .set_ca_file("test/root-ca.pem"));

                        client::Options::default().tls(Context {
                            domain: DOMAIN.into(),
                            tls_connector: unwrap!(connector.build()),
                        })
                    }
                // not implemented for windows or other platforms
                } else {
                    fn get_tls_client_context() -> Context {
                        unimplemented!()
                    }
                }
            }

            fn get_sync_client<C>(addr: SocketAddr) -> io::Result<C>
                where C: client::sync::Connect
            {
                let client_options = get_tls_client_options();
                C::connect(addr, client_options)
            }

            fn start_server_with_sync_client<C, S>(server: S) -> (SocketAddr, io::Result<C>)
                where C: client::sync::Connect, S: SyncServiceExt
            {
                let (server_options, client_options) = tls_context();
                let addr = unwrap!(server.listen("localhost:0".first_socket_addr(),
                                                 server_options));
                let client = C::connect(addr, client_options);
                (addr, client)
            }

            fn start_server_with_async_client<C, S>(server: S) -> (SocketAddr, C)
                where C: client::future::Connect, S: FutureServiceExt
            {
                let (server_options, client_options) = tls_context();
                let addr = unwrap!(server.listen("localhost:0".first_socket_addr(),
                              server_options).wait());
                let client = unwrap!(C::connect(addr, client_options).wait());
                (addr, client)
            }

            fn start_err_server_with_async_client<C, S>(server: S) -> (SocketAddr, C)
                where C: client::future::Connect, S: error_service::FutureServiceExt
            {
                let (server_options, client_options) = tls_context();
                let addr = unwrap!(server.listen("localhost:0".first_socket_addr(),
                              server_options).wait());
                let client = unwrap!(C::connect(addr, client_options).wait());
                (addr, client)
            }
        } else {
            fn get_server_options() -> server::Options {
                server::Options::default()
            }

            fn get_client_options() -> client::Options {
                client::Options::default()
            }

            fn get_sync_client<C>(addr: SocketAddr) -> io::Result<C>
                where C: client::sync::Connect
            {
                C::connect(addr, get_client_options())
            }

            fn start_server_with_sync_client<C, S>(server: S) -> (SocketAddr, io::Result<C>)
                where C: client::sync::Connect, S: SyncServiceExt
            {
                let addr = unwrap!(server.listen("localhost:0".first_socket_addr(),
                              get_server_options()));
                let client = C::connect(addr, get_client_options());
                (addr, client)
            }

            fn start_server_with_async_client<C, S>(server: S) -> (SocketAddr, C)
                where C: client::future::Connect, S: FutureServiceExt
            {
                let addr = unwrap!(server.listen("localhost:0".first_socket_addr(),
                              get_server_options()).wait());
                let client = unwrap!(C::connect(addr, get_client_options()).wait());
                (addr, client)
            }

            fn start_err_server_with_async_client<C, S>(server: S) -> (SocketAddr, C)
                where C: client::future::Connect, S: error_service::FutureServiceExt
            {
                let addr = unwrap!(server.listen("localhost:0".first_socket_addr(),
                              get_server_options()).wait());
                let client = unwrap!(C::connect(addr, get_client_options()).wait());
                (addr, client)
            }
        }
    }


    mod sync {
        use super::{SyncClient, SyncService, env_logger, start_server_with_sync_client};
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
            let (_, client) = start_server_with_sync_client::<SyncClient, Server>(Server);
            let client = unwrap!(client);
            assert_eq!(3, client.add(1, 2).unwrap());
            assert_eq!("Hey, Tim.", client.hey("Tim".to_string()).unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::init();
            let (_, client) = start_server_with_sync_client::<super::other_service::SyncClient,
                                                              Server>(Server);
            let client = client.expect("Could not connect!");
            match client.foo().err().expect("failed unwrap") {
                ::Error::ServerDeserialize(_) => {} // good
                bad => panic!("Expected Error::ServerDeserialize but got {}", bad),
            }
        }
    }

    mod future {
        use futures::{Finished, Future, finished};
        use super::{FutureClient, FutureService, env_logger, start_server_with_async_client};
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
            let (_, client) = start_server_with_async_client::<FutureClient, Server>(Server);
            assert_eq!(3, client.add(1, 2).wait().unwrap());
            assert_eq!("Hey, Tim.", client.hey("Tim".to_string()).wait().unwrap());
        }

        #[test]
        fn concurrent() {
            let _ = env_logger::init();
            let (_, client) = start_server_with_async_client::<FutureClient, Server>(Server);
            let req1 = client.add(1, 2);
            let req2 = client.add(3, 4);
            let req3 = client.hey("Tim".to_string());
            assert_eq!(3, req1.wait().unwrap());
            assert_eq!(7, req2.wait().unwrap());
            assert_eq!("Hey, Tim.", req3.wait().unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::init();
            let (_, client) =
                start_server_with_async_client::<super::other_service::FutureClient,
                                                 Server>(Server);
            match client.foo().wait().err().unwrap() {
                ::Error::ServerDeserialize(_) => {} // good
                bad => panic!(r#"Expected Error::ServerDeserialize but got "{}""#, bad),
            }
        }

        #[cfg(feature = "tls")]
        #[test]
        fn tcp_and_tls() {
            use {client, server};
            use util::FirstSocketAddr;
            use client::future::Connect;
            use super::FutureServiceExt;

            let _ = env_logger::init();
            let (_, client) = start_server_with_async_client::<FutureClient, Server>(Server);
            assert_eq!(3, client.add(1, 2).wait().unwrap());
            assert_eq!("Hey, Tim.", client.hey("Tim".to_string()).wait().unwrap());

            let addr = Server.listen("localhost:0".first_socket_addr(),
                        server::Options::default())
                .wait()
                .unwrap();
            let client = FutureClient::connect(addr, client::Options::default()).wait().unwrap();
            assert_eq!(3, client.add(1, 2).wait().unwrap());
            assert_eq!("Hey, Tim.", client.hey("Tim".to_string()).wait().unwrap());
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
        use std::error::Error as E;
        use self::error_service::*;
        let _ = env_logger::init();

        let (addr, client) = start_err_server_with_async_client::<FutureClient,
                                                                  ErrorServer>(ErrorServer);
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

        let client = get_sync_client::<SyncClient>(addr).unwrap();
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
