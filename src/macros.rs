// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[doc(hidden)]
#[macro_export]
macro_rules! as_item {
    ($i:item) => {$i};
}

/// The main macro that creates RPC services.
///
/// Rpc methods are specified, mirroring trait syntax:
///
/// ```
/// # #![feature(plugin, use_extern_macros, proc_macro_path_invoc)]
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

        #[doc(hidden)]
        #[allow(non_camel_case_types, unused)]
        #[derive($crate::serde_derive::Serialize, $crate::serde_derive::Deserialize)]
        pub enum Request__ {
            $(
                $fn_name{ $($arg: $in_,)* }
            ),*
        }

        #[doc(hidden)]
        #[allow(non_camel_case_types, unused)]
        #[derive($crate::serde_derive::Serialize, $crate::serde_derive::Deserialize)]
        pub enum Response__ {
            $(
                $fn_name($out)
            ),*
        }

        #[doc(hidden)]
        #[allow(non_camel_case_types, unused)]
        #[derive(Debug, $crate::serde_derive::Deserialize, $crate::serde_derive::Serialize)]
        pub enum Error__ {
            $(
                $fn_name($error)
            ),*
        }

/// Defines the `Future` RPC service. Implementors must be `Clone` and `'static`,
/// as required by `tokio_proto::NewService`. This is required so that the service can be used
/// to respond to multiple requests concurrently.
        pub trait FutureService:
            ::std::clone::Clone +
            'static
        {
            $(
                snake_to_camel! {
                    /// The type of future returned by `{}`.
                    type $fn_name: $crate::futures::IntoFuture<Item=$out, Error=$error>;
                }

                $(#[$attr])*
                fn $fn_name(&self, $($arg:$in_),*) -> ty_snake_to_camel!(Self::$fn_name);
            )*
        }

        #[allow(non_camel_case_types)]
        #[derive(Clone)]
        struct TarpcNewService<S>(S);

        impl<S> ::std::fmt::Debug for TarpcNewService<S> {
            fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                fmt.debug_struct("TarpcNewService").finish()
            }
        }

        #[allow(non_camel_case_types)]
        type ResponseFuture__ =
            $crate::futures::Finished<$crate::future::server::Response<Response__,
                                                       Error__>,
                                      ::std::io::Error>;

        #[allow(non_camel_case_types)]
        enum FutureReply__<S__: FutureService> {
            DeserializeError(ResponseFuture__),
            $($fn_name(
                    $crate::futures::Then<
                        <ty_snake_to_camel!(S__::$fn_name) as $crate::futures::IntoFuture>::Future,
                        ResponseFuture__,
                        fn(::std::result::Result<$out, $error>) -> ResponseFuture__>)),*
        }

        impl<S: FutureService> $crate::futures::Future for FutureReply__<S> {
            type Item = $crate::future::server::Response<Response__, Error__>;
            type Error = ::std::io::Error;

            fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                match *self {
                    FutureReply__::DeserializeError(ref mut future__) => {
                        $crate::futures::Future::poll(future__)
                    }
                    $(
                        FutureReply__::$fn_name(ref mut future__) => {
                            $crate::futures::Future::poll(future__)
                        }
                    ),*
                }
            }
        }


        #[allow(non_camel_case_types)]
        impl<S__> $crate::tokio_service::Service for TarpcNewService<S__>
            where S__: FutureService
        {
            type Request = ::std::result::Result<Request__, $crate::bincode::Error>;
            type Response = $crate::future::server::Response<Response__, Error__>;
            type Error = ::std::io::Error;
            type Future = FutureReply__<S__>;

            fn call(&self, request__: Self::Request) -> Self::Future {
                let request__ = match request__ {
                    Ok(request__) => request__,
                    Err(err__) => {
                        return FutureReply__::DeserializeError(
                            $crate::futures::finished(
                                ::std::result::Result::Err(
                                    $crate::WireError::RequestDeserialize(
                                        ::std::string::ToString::to_string(&err__)))));
                    }
                };
                #[allow(unreachable_patterns)]
                match request__ {
                    $(
                        Request__::$fn_name{ $($arg,)* } => {
                            fn wrap__(response__: ::std::result::Result<$out, $error>)
                                -> ResponseFuture__
                            {
                                $crate::futures::finished(
                                    response__
                                        .map(Response__::$fn_name)
                                        .map_err(|err__| {
                                            $crate::WireError::App(Error__::$fn_name(err__))
                                        })
                                )
                            }
                            return FutureReply__::$fn_name(
                                $crate::futures::Future::then(
                                        $crate::futures::IntoFuture::into_future(
                                            FutureService::$fn_name(&self.0, $($arg),*)),
                                        wrap__));
                        }
                    )*
                    _ => unreachable!(),
                }
            }
        }

        #[allow(non_camel_case_types)]
        impl<S__> $crate::tokio_service::NewService
            for TarpcNewService<S__>
            where S__: FutureService
        {
            type Request = <Self as $crate::tokio_service::Service>::Request;
            type Response = <Self as $crate::tokio_service::Service>::Response;
            type Error = <Self as $crate::tokio_service::Service>::Error;
            type Instance = Self;

            fn new_service(&self) -> ::std::io::Result<Self> {
                Ok(self.clone())
            }
        }

        /// The future returned by `FutureServiceExt::listen`.
        #[allow(unused)]
        pub struct Listen<S>
            where S: FutureService,
        {
            inner: $crate::future::server::Listen<TarpcNewService<S>,
                                          Request__,
                                          Response__,
                                          Error__>,
        }

        impl<S> $crate::futures::Future for Listen<S>
            where S: FutureService
        {
            type Item = ();
            type Error = ();

            fn poll(&mut self) -> $crate::futures::Poll<(), ()> {
                self.inner.poll()
            }
        }

        /// Provides a function for starting the service. This is a separate trait from
        /// `FutureService` to prevent collisions with the names of RPCs.
        pub trait FutureServiceExt: FutureService {
            /// Spawns the service, binding to the given address and running on
            /// the `reactor::Core` associated with `handle`.
            ///
            /// Returns the address being listened on as well as the server future. The future
            /// must be executed for the server to run.
            fn listen(self,
                      addr: ::std::net::SocketAddr,
                      handle: &$crate::tokio_core::reactor::Handle,
                      options: $crate::future::server::Options)
                -> ::std::io::Result<($crate::future::server::Handle, Listen<Self>)>
            {
                $crate::future::server::listen(TarpcNewService(self),
                                               addr,
                                               handle,
                                               options)
                    .map(|(handle, inner)| (handle, Listen { inner }))
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
            /// Spawns the service, binding to the given address and returning the server handle.
            ///
            /// To actually run the server, call `run` on the returned handle.
            fn listen<A>(self, addr: A, options: $crate::sync::server::Options)
                -> ::std::io::Result<$crate::sync::server::Handle>
                    where A: ::std::net::ToSocketAddrs
            {
                #[derive(Clone)]
                struct BlockingFutureService<S>(S);
                impl<S: SyncService> FutureService for BlockingFutureService<S> {
                    $(
                        impl_snake_to_camel! {
                            type $fn_name =
                                $crate::util::Lazy<
                                    fn((S, $($in_),*)) -> ::std::result::Result<$out, $error>,
                                    (S, $($in_),*),
                                    ::std::result::Result<$out, $error>>;
                        }

                        $(#[$attr])*
                        fn $fn_name(&self, $($arg:$in_),*)
                        -> $crate::util::Lazy<
                               fn((S, $($in_),*)) -> ::std::result::Result<$out, $error>,
                               (S, $($in_),*),
                               ::std::result::Result<$out, $error>> {
                            fn execute<S: SyncService>((s, $($arg),*): (S, $($in_),*))
                                -> ::std::result::Result<$out, $error> {
                              SyncService::$fn_name(&s, $($arg),*)
                            }
                            $crate::util::lazy(execute, (self.0.clone(), $($arg),*))
                        }
                    )*
                }

                let tarpc_service__ = TarpcNewService(BlockingFutureService(self));
                let addr__ = $crate::util::FirstSocketAddr::try_first_socket_addr(&addr)?;
                return $crate::sync::server::listen(tarpc_service__, addr__, options);
            }
        }

        impl<A> FutureServiceExt for A where A: FutureService {}
        impl<S> SyncServiceExt for S where S: SyncService {}

        /// The client stub that makes RPC calls to the server. Exposes a blocking interface.
        #[allow(unused)]
        #[derive(Clone, Debug)]
        pub struct SyncClient {
            inner: SyncClient__,
        }

        impl $crate::sync::client::ClientExt for SyncClient {
            fn connect<A>(addr_: A, options_: $crate::sync::client::Options)
                -> ::std::io::Result<Self>
                where A: ::std::net::ToSocketAddrs,
            {
                let client_ =
                    <SyncClient__ as $crate::sync::client::ClientExt>::connect(addr_, options_)?;
                ::std::result::Result::Ok(SyncClient {
                    inner: client_,
                })
            }
        }

        impl SyncClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                pub fn $fn_name(&self, $($arg: $in_),*)
                    -> ::std::result::Result<$out, $crate::Error<$error>>
                {
                    tarpc_service_then__!($out, $error, $fn_name);
                    let resp__ = self.inner.call(Request__::$fn_name { $($arg,)* });
                    tarpc_service_then__(resp__)
                }
            )*
        }

        #[allow(non_camel_case_types)]
        type FutureClient__ = $crate::future::client::Client<Request__, Response__, Error__>;

        #[allow(non_camel_case_types)]
        type SyncClient__ = $crate::sync::client::Client<Request__, Response__, Error__>;

        #[allow(non_camel_case_types)]
        /// A future representing a client connecting to a server.
        pub struct Connect<T> {
            inner:
                $crate::futures::Map<
                    $crate::future::client::ConnectFuture< Request__, Response__, Error__>,
                    fn(FutureClient__) -> T>,
        }

        impl<T> $crate::futures::Future for Connect<T> {
            type Item = T;
            type Error = ::std::io::Error;

            fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                $crate::futures::Future::poll(&mut self.inner)
            }
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct FutureClient(FutureClient__);

        impl<'a> $crate::future::client::ClientExt for FutureClient {
            type ConnectFut = Connect<Self>;

            fn connect(addr__: ::std::net::SocketAddr,
                                options__: $crate::future::client::Options)
                -> Self::ConnectFut
            {
                let client =
                    <FutureClient__ as $crate::future::client::ClientExt>::connect(addr__,
                                                                                   options__);

                Connect {
                    inner: $crate::futures::Future::map(client, FutureClient)
                }
            }
        }

        impl FutureClient {
            $(
                #[allow(unused)]
                $(#[$attr])*
                pub fn $fn_name(&self, $($arg: $in_),*)
                    -> $crate::futures::future::Then<
                           <FutureClient__ as $crate::tokio_service::Service>::Future,
                           ::std::result::Result<$out, $crate::Error<$error>>,
                           fn(::std::result::Result<Response__,
                                                    $crate::Error<Error__>>)
                           -> ::std::result::Result<$out, $crate::Error<$error>>> {
                    tarpc_service_then__!($out, $error, $fn_name);

                    let request__ = Request__::$fn_name { $($arg,)* };
                    let future__ = $crate::tokio_service::Service::call(&self.0, request__);
                    return $crate::futures::Future::then(future__, tarpc_service_then__);
                }
            )*
        }
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! tarpc_service_then__ {
    ($out:ty, $error:ty, $fn_name:ident) => {
        fn tarpc_service_then__(msg__:
                      ::std::result::Result<Response__,
                                            $crate::Error<Error__>>)
                  -> ::std::result::Result<$out, $crate::Error<$error>> {
            match msg__ {
                ::std::result::Result::Ok(msg__) => {
                    #[allow(unreachable_patterns)]
                    match msg__ {
                        Response__::$fn_name(msg__) =>
                            ::std::result::Result::Ok(msg__),
                        _ => unreachable!(),
                    }
                }
                ::std::result::Result::Err(err__) => {
                    ::std::result::Result::Err(match err__ {
                        $crate::Error::App(err__) => {
                            #[allow(unreachable_patterns)]
                            match err__ {
                                Error__::$fn_name(err__) =>
                                    $crate::Error::App(err__),
                                _ => unreachable!(),
                            }
                        }
                        $crate::Error::RequestDeserialize(err__) => {
                            $crate::Error::RequestDeserialize(err__)
                        }
                        $crate::Error::ResponseDeserialize(err__) => {
                            $crate::Error::ResponseDeserialize(err__)
                        }
                        $crate::Error::Io(err__) => {
                            $crate::Error::Io(err__)
                        }
                    })
                }
            }
        }
    };
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
    use {sync, future};
    use futures::{Future, failed};
    use std::io;
    use std::net::SocketAddr;
    use tokio_core::reactor;
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
            const DOMAIN: &str = "foobar.com";

            use tls::client::Context;
            use native_tls::{Pkcs12, TlsAcceptor, TlsConnector};

            fn get_tls_acceptor() -> TlsAcceptor {
                let buf = include_bytes!("../test/identity.p12");
                let pkcs12 = unwrap!(Pkcs12::from_der(buf, "mypass"));
                unwrap!(unwrap!(TlsAcceptor::builder(pkcs12)).build())
            }

            fn get_future_tls_server_options() -> future::server::Options {
                future::server::Options::default().tls(get_tls_acceptor())
            }

            fn get_sync_tls_server_options() -> sync::server::Options {
                sync::server::Options::default().tls(get_tls_acceptor())
            }

            // Making the TlsConnector for testing needs to be OS-dependent just like native-tls.
            // We need to go through this trickery because the test self-signed cert is not part
            // of the system's cert chain. If it was, then all that is required is
            // `TlsConnector::builder().unwrap().build().unwrap()`.
            cfg_if! {
                if #[cfg(target_os = "macos")] {
                    extern crate security_framework;

                    use native_tls_inner::Certificate;

                    fn get_future_tls_client_options() -> future::client::Options {
                        future::client::Options::default().tls(get_tls_client_context())
                    }

                    fn get_sync_tls_client_options() -> sync::client::Options {
                        sync::client::Options::default().tls(get_tls_client_context())
                    }

                    fn get_tls_client_context() -> Context {
                        let buf = include_bytes!("../test/root-ca.der");
                        let cert = unwrap!(Certificate::from_der(buf));
                        let mut connector = unwrap!(TlsConnector::builder());
                        connector.add_root_certificate(cert).unwrap();

                        Context {
                            domain: DOMAIN.into(),
                            tls_connector: unwrap!(connector.build()),
                        }
                    }
                } else if #[cfg(all(not(target_os = "macos"), not(windows)))] {
                    use native_tls_inner::backend::openssl::TlsConnectorBuilderExt;

                    fn get_sync_tls_client_options() -> sync::client::Options {
                        sync::client::Options::default()
                            .tls(get_tls_client_context())
                    }

                    fn get_future_tls_client_options() -> future::client::Options {
                        future::client::Options::default()
                            .tls(get_tls_client_context())
                    }

                    fn get_tls_client_context() -> Context {
                        let mut connector = unwrap!(TlsConnector::builder());
                        unwrap!(connector.builder_mut()
                           .set_ca_file("test/root-ca.pem"));
                        Context {
                            domain: DOMAIN.into(),
                            tls_connector: unwrap!(connector.build()),
                        }
                    }
                // not implemented for windows or other platforms
                } else {
                    fn get_tls_client_context() -> Context {
                        unimplemented!()
                    }
                }
            }

            fn get_sync_client<C>(addr: SocketAddr) -> io::Result<C>
                where C: sync::client::ClientExt
            {
                C::connect(addr, get_sync_tls_client_options())
            }

            fn get_future_client<C>(addr: SocketAddr, handle: reactor::Handle) -> C::ConnectFut
                where C: future::client::ClientExt
            {
                C::connect(addr, get_future_tls_client_options().handle(handle))
            }

            fn start_server_with_sync_client<C, S>(server: S)
                -> io::Result<(SocketAddr, C, future::server::Shutdown)>
                where C: sync::client::ClientExt, S: SyncServiceExt
            {
                let options = get_sync_tls_server_options();
                let (tx, rx) = ::std::sync::mpsc::channel();
                ::std::thread::spawn(move || {
                    let handle = unwrap!(server.listen("localhost:0".first_socket_addr(),
                                                           options));
                    tx.send((handle.addr(), handle.shutdown())).unwrap();
                    handle.run();
                });
                let (addr, shutdown) = rx.recv().unwrap();
                let client = unwrap!(C::connect(addr, get_sync_tls_client_options()));
                Ok((addr, client, shutdown))
            }

            fn start_server_with_async_client<C, S>(server: S)
                -> io::Result<(future::server::Handle, reactor::Core, C)>
                where C: future::client::ClientExt, S: FutureServiceExt
            {
                let mut reactor = reactor::Core::new()?;
                let server_options = get_future_tls_server_options();
                let (handle, server) = server.listen("localhost:0".first_socket_addr(),
                                                   &reactor.handle(),
                                                   server_options)?;
                reactor.handle().spawn(server);
                let client_options = get_future_tls_client_options().handle(reactor.handle());
                let client = unwrap!(reactor.run(C::connect(handle.addr(), client_options)));
                Ok((handle, reactor, client))
            }

            fn return_server<S>(server: S)
                -> io::Result<(future::server::Handle, reactor::Core, Listen<S>)>
                where S: FutureServiceExt
            {
                let reactor = reactor::Core::new()?;
                let server_options = get_future_tls_server_options();
                let (handle, server) = server.listen("localhost:0".first_socket_addr(),
                                                   &reactor.handle(),
                                                   server_options)?;
                Ok((handle, reactor, server))
            }

            fn start_err_server_with_async_client<C, S>(server: S)
                -> io::Result<(future::server::Handle, reactor::Core, C)>
                where C: future::client::ClientExt, S: error_service::FutureServiceExt
            {
                let mut reactor = reactor::Core::new()?;
                let server_options = get_future_tls_server_options();
                let (handle, server) = server.listen("localhost:0".first_socket_addr(),
                                                   &reactor.handle(),
                                                   server_options)?;
                reactor.handle().spawn(server);
                let client_options = get_future_tls_client_options().handle(reactor.handle());
                let client = unwrap!(reactor.run(C::connect(handle.addr(), client_options)));
                Ok((handle, reactor, client))
            }
        } else {
            fn get_future_server_options() -> future::server::Options {
                future::server::Options::default()
            }

            fn get_sync_server_options() -> sync::server::Options {
                sync::server::Options::default()
            }

            fn get_future_client_options() -> future::client::Options {
                future::client::Options::default()
            }

            fn get_sync_client_options() -> sync::client::Options {
                sync::client::Options::default()
            }

            fn get_sync_client<C>(addr: SocketAddr) -> io::Result<C>
                where C: sync::client::ClientExt
            {
                C::connect(addr, get_sync_client_options())
            }

            fn get_future_client<C>(addr: SocketAddr, handle: reactor::Handle) -> C::ConnectFut
                where C: future::client::ClientExt
            {
                C::connect(addr, get_future_client_options().handle(handle))
            }

            fn start_server_with_sync_client<C, S>(server: S)
                -> io::Result<(SocketAddr, C, future::server::Shutdown)>
                where C: sync::client::ClientExt, S: SyncServiceExt
            {
                let options = get_sync_server_options();
                let (tx, rx) = ::std::sync::mpsc::channel();
                ::std::thread::spawn(move || {
                    let handle = unwrap!(server.listen("localhost:0".first_socket_addr(), options));
                    tx.send((handle.addr(), handle.shutdown())).unwrap();
                    handle.run();
                });
                let (addr, shutdown) = rx.recv().unwrap();
                let client = unwrap!(get_sync_client(addr));
                Ok((addr, client, shutdown))
            }

            fn start_server_with_async_client<C, S>(server: S)
                -> io::Result<(future::server::Handle, reactor::Core, C)>
                where C: future::client::ClientExt, S: FutureServiceExt
            {
                let mut reactor = reactor::Core::new()?;
                let options = get_future_server_options();
                let (handle, server) = server.listen("localhost:0".first_socket_addr(),
                                         &reactor.handle(),
                                         options)?;
                reactor.handle().spawn(server);
                let client = unwrap!(reactor.run(C::connect(handle.addr(),
                                                            get_future_client_options())));
                Ok((handle, reactor, client))
            }

            fn return_server<S>(server: S)
                -> io::Result<(future::server::Handle, reactor::Core, Listen<S>)>
                where S: FutureServiceExt
            {
                let reactor = reactor::Core::new()?;
                let options = get_future_server_options();
                let (handle, server) = server.listen("localhost:0".first_socket_addr(),
                                                   &reactor.handle(),
                                                   options)?;
                Ok((handle, reactor, server))
            }

            fn start_err_server_with_async_client<C, S>(server: S)
                -> io::Result<(future::server::Handle, reactor::Core, C)>
                where C: future::client::ClientExt, S: error_service::FutureServiceExt
            {
                let mut reactor = reactor::Core::new()?;
                let options = get_future_server_options();
                let (handle, server) = server.listen("localhost:0".first_socket_addr(),
                                         &reactor.handle(),
                                         options)?;
                reactor.handle().spawn(server);
                let client = C::connect(handle.addr(), get_future_client_options());
                let client = unwrap!(reactor.run(client));
                Ok((handle, reactor, client))
            }
        }
    }

    mod sync_tests {
        use super::{SyncClient, SyncService, get_sync_client, env_logger,
                    start_server_with_sync_client};
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
            let _ = env_logger::try_init();
            let (_, client, _) =
                unwrap!(start_server_with_sync_client::<SyncClient, Server>(Server));
            assert_eq!(3, client.add(1, 2).unwrap());
            assert_eq!("Hey, Tim.", client.hey("Tim".to_string()).unwrap());
        }

        #[test]
        fn shutdown() {
            use futures::{Async, Future};

            let _ = env_logger::try_init();
            let (addr, client, shutdown) =
                unwrap!(start_server_with_sync_client::<SyncClient, Server>(Server));
            assert_eq!(3, unwrap!(client.add(1, 2)));
            assert_eq!("Hey, Tim.", unwrap!(client.hey("Tim".to_string())));

            info!("Dropping client.");
            drop(client);
            let (tx, rx) = ::std::sync::mpsc::channel();
            let (tx2, rx2) = ::std::sync::mpsc::channel();
            let shutdown2 = shutdown.clone();
            ::std::thread::spawn(move || {
                let client = unwrap!(get_sync_client::<SyncClient>(addr));
                let add = unwrap!(client.add(3, 2));
                unwrap!(tx.send(()));
                drop(client);
                // Make sure 2 shutdowns are concurrent safe.
                unwrap!(shutdown2.shutdown().wait());
                unwrap!(tx2.send(add));
            });
            unwrap!(rx.recv());
            let mut shutdown1 = shutdown.shutdown();
            unwrap!(shutdown.shutdown().wait());
            // Assert shutdown2 blocks until shutdown is complete.
            if let Async::NotReady = unwrap!(shutdown1.poll()) {
                panic!("Shutdown should have completed");
            }
            // Existing clients are served
            assert_eq!(5, unwrap!(rx2.recv()));

            let e = get_sync_client::<SyncClient>(addr).err().unwrap();
            debug!("(Success) shutdown caused client err: {}", e);
        }

        #[test]
        fn no_shutdown() {
            let _ = env_logger::try_init();
            let (addr, client, shutdown) =
                unwrap!(start_server_with_sync_client::<SyncClient, Server>(Server));
            assert_eq!(3, client.add(1, 2).unwrap());
            assert_eq!("Hey, Tim.", client.hey("Tim".to_string()).unwrap());

            drop(shutdown);

            // Existing clients are served.
            assert_eq!(3, client.add(1, 2).unwrap());
            // New connections are accepted.
            assert!(get_sync_client::<SyncClient>(addr).is_ok());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::try_init();
            let (_, client, _) = unwrap!(start_server_with_sync_client::<
                super::other_service::SyncClient,
                Server,
            >(Server));
            match client.foo().err().expect("failed unwrap") {
                ::Error::RequestDeserialize(_) => {} // good
                bad => panic!("Expected Error::RequestDeserialize but got {}", bad),
            }
        }
    }

    mod bad_serialize {
        use serde::{Serialize, Serializer};
        use serde::ser::SerializeSeq;
        use sync::{client, server};
        use sync::client::ClientExt;

        #[derive(Deserialize)]
        pub struct Bad;

        impl Serialize for Bad {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_seq(None)?.end()
            }
        }

        service! {
            rpc bad(bad: Bad) | ();
        }

        impl SyncService for () {
            fn bad(&self, _: Bad) -> Result<(), ()> {
                Ok(())
            }
        }

        #[test]
        fn bad_serialize() {
            let handle = ()
                .listen("localhost:0", server::Options::default())
                .unwrap();
            let client = SyncClient::connect(handle.addr(), client::Options::default()).unwrap();
            client.bad(Bad).err().unwrap();
        }
    }

    mod future_tests {
        use super::{FutureClient, FutureService, env_logger, get_future_client, return_server,
                    start_server_with_async_client};
        use futures::{Finished, finished};
        use tokio_core::reactor;
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
            let _ = env_logger::try_init();
            let (_, mut reactor, client) = unwrap!(
                start_server_with_async_client::<FutureClient, Server>(Server)
            );
            assert_eq!(3, reactor.run(client.add(1, 2)).unwrap());
            assert_eq!(
                "Hey, Tim.",
                reactor.run(client.hey("Tim".to_string())).unwrap()
            );
        }

        #[test]
        fn shutdown() {
            use futures::Future;
            use tokio_core::reactor;

            let _ = env_logger::try_init();
            let (handle, mut reactor, server) = unwrap!(return_server::<Server>(Server));

            let (tx, rx) = ::std::sync::mpsc::channel();
            ::std::thread::spawn(move || {
                let mut reactor = reactor::Core::new().unwrap();
                let client = get_future_client::<FutureClient>(handle.addr(), reactor.handle());
                let client = reactor.run(client).unwrap();
                let add = reactor.run(client.add(3, 2)).unwrap();
                assert_eq!(add, 5);
                trace!("Dropping client.");
                drop(reactor);
                debug!("Shutting down...");
                handle.shutdown().shutdown().wait().unwrap();
                tx.send(add).unwrap();
            });
            reactor.run(server).unwrap();
            assert_eq!(rx.recv().unwrap(), 5);
        }

        #[test]
        fn concurrent() {
            let _ = env_logger::try_init();
            let (_, mut reactor, client) = unwrap!(
                start_server_with_async_client::<FutureClient, Server>(Server)
            );
            let req1 = client.add(1, 2);
            let req2 = client.add(3, 4);
            let req3 = client.hey("Tim".to_string());
            assert_eq!(3, reactor.run(req1).unwrap());
            assert_eq!(7, reactor.run(req2).unwrap());
            assert_eq!("Hey, Tim.", reactor.run(req3).unwrap());
        }

        #[test]
        fn other_service() {
            let _ = env_logger::try_init();
            let (_, mut reactor, client) = unwrap!(start_server_with_async_client::<
                super::other_service::FutureClient,
                Server,
            >(Server));
            match reactor.run(client.foo()).err().unwrap() {
                ::Error::RequestDeserialize(_) => {} // good
                bad => panic!(r#"Expected Error::RequestDeserialize but got "{}""#, bad),
            }
        }

        #[test]
        fn reuse_addr() {
            use util::FirstSocketAddr;
            use future::server;
            use super::FutureServiceExt;

            let _ = env_logger::try_init();
            let reactor = reactor::Core::new().unwrap();
            let handle = Server
                .listen(
                    "localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default(),
                )
                .unwrap()
                .0;
            Server
                .listen(handle.addr(), &reactor.handle(), server::Options::default())
                .unwrap();
        }

        #[test]
        fn drop_client() {
            use future::{client, server};
            use future::client::ClientExt;
            use util::FirstSocketAddr;
            use super::{FutureClient, FutureServiceExt};

            let _ = env_logger::try_init();
            let mut reactor = reactor::Core::new().unwrap();
            let (handle, server) = Server
                .listen(
                    "localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default(),
                )
                .unwrap();
            reactor.handle().spawn(server);

            let client = FutureClient::connect(
                handle.addr(),
                client::Options::default().handle(reactor.handle()),
            );
            let client = unwrap!(reactor.run(client));
            assert_eq!(reactor.run(client.add(1, 2)).unwrap(), 3);
            drop(client);

            let client = FutureClient::connect(
                handle.addr(),
                client::Options::default().handle(reactor.handle()),
            );
            let client = unwrap!(reactor.run(client));
            assert_eq!(reactor.run(client.add(1, 2)).unwrap(), 3);
        }

        #[cfg(feature = "tls")]
        #[test]
        fn tcp_and_tls() {
            use future::{client, server};
            use util::FirstSocketAddr;
            use future::client::ClientExt;
            use super::FutureServiceExt;

            let _ = env_logger::try_init();
            let (_, mut reactor, client) = unwrap!(
                start_server_with_async_client::<FutureClient, Server>(Server)
            );
            assert_eq!(3, reactor.run(client.add(1, 2)).unwrap());
            assert_eq!(
                "Hey, Tim.",
                reactor.run(client.hey("Tim".to_string())).unwrap()
            );

            let (handle, server) = Server
                .listen(
                    "localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default(),
                )
                .unwrap();
            reactor.handle().spawn(server);
            let options = client::Options::default().handle(reactor.handle());
            let client = reactor
                .run(FutureClient::connect(handle.addr(), options))
                .unwrap();
            assert_eq!(3, reactor.run(client.add(1, 2)).unwrap());
            assert_eq!(
                "Hey, Tim.",
                reactor.run(client.hey("Tim".to_string())).unwrap()
            );
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
        let _ = env_logger::try_init();

        let (_, mut reactor, client) =
            start_err_server_with_async_client::<FutureClient, ErrorServer>(ErrorServer).unwrap();
        reactor
            .run(client.bar().then(move |result| {
                match result.err().unwrap() {
                    ::Error::App(e) => {
                        assert_eq!(e.description(), "lol jk");
                        Ok::<_, ()>(())
                    } // good
                    bad => panic!("Expected Error::App but got {:?}", bad),
                }
            }))
            .unwrap();
    }

    pub mod other_service {
        service! {
            rpc foo();
        }
    }
}
