// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[doc(hidden)]
#[macro_export]
macro_rules! as_item {
    ($i:item) => {
        $i
    };
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
/// * `FutureServiceExt` -- provides the methods for starting a service. There is an umbrella impl
///                         for all implers of `FutureService`. It's a separate trait to prevent
///                         name collisions with RPCs.
/// * `FutureClient` -- a client whose RPCs return `Future`s.
///
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
            rpc $fn_name( $( $arg : $in_ ),* ) -> ();
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
            rpc $fn_name( $( $arg : $in_ ),* ) -> $out;
        }
    };
// Pattern for when the next rpc has an implicit unit return type and an explicit error type.
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
// Pattern for when the next rpc has an explicit return type and an explicit error type.
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
                    type $fn_name: $crate::futures::Future<Item = $out, Error = ()>;
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
            $crate::futures::Finished<$crate::future::server::Response<Response__>,
                                      ::std::io::Error>;

        #[allow(non_camel_case_types)]
        enum FutureReply__<S__: FutureService> {
            DeserializeError(ResponseFuture__),
            $($fn_name(
                    $crate::futures::Map<
                        ty_snake_to_camel!(S__::$fn_name),
                        fn($out) -> $crate::future::server::Response<Response__>,>)),*
        }

        impl<S: FutureService> $crate::futures::Future for FutureReply__<S> {
            type Item = $crate::future::server::Response<Response__>;
            type Error = ::std::io::Error;

            fn poll(&mut self) -> $crate::futures::Poll<Self::Item, Self::Error> {
                match *self {
                    FutureReply__::DeserializeError(ref mut future__) => {
                        $crate::futures::Future::poll(future__)
                    }
                    $(
                        FutureReply__::$fn_name(ref mut future__) => {
                            $crate::futures::Future::poll(future__).map_err(|()| unreachable!())
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
            type Response = $crate::future::server::Response<Response__>;
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
                match request__ {
                    $(
                        Request__::$fn_name{ $($arg,)* } => {
                            fn wrap__(response__: $out)
                                -> $crate::future::server::Response<Response__>
                            {
                                Ok(Response__::$fn_name(response__))
                            }
                            return FutureReply__::$fn_name(
                                $crate::futures::Future::map(
                                            FutureService::$fn_name(&self.0, $($arg),*),
                                        wrap__));
                        }
                    )*
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
                                          Response__>,
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

        impl<A> FutureServiceExt for A where A: FutureService {}

        #[allow(non_camel_case_types)]
        type FutureClient__ = $crate::future::client::Client<Request__, Response__>;

        #[allow(non_camel_case_types)]
        /// A future representing a client connecting to a server.
        pub struct Connect<T> {
            inner:
                $crate::futures::Map<
                    $crate::future::client::ConnectFuture< Request__, Response__>,
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
                           ::std::result::Result<$out, $crate::Error>,
                           fn(::std::result::Result<Response__,
                                                    $crate::Error>)
                           -> ::std::result::Result<$out, $crate::Error>> {
                    tarpc_service_then__!($out, $fn_name);

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
    ($out:ty, $fn_name:ident) => {
        fn tarpc_service_then__(
            msg__: ::std::result::Result<Response__, $crate::Error>,
        ) -> ::std::result::Result<$out, $crate::Error> {
            match msg__ {
                ::std::result::Result::Ok(msg__) =>
                match msg__ {
                    Response__::$fn_name(msg__) => ::std::result::Result::Ok(msg__),
                    _ => unreachable!(),
                },
                ::std::result::Result::Err(err__) => ::std::result::Result::Err(match err__ {
                    $crate::Error::RequestDeserialize(err__) => {
                        $crate::Error::RequestDeserialize(err__)
                    }
                    $crate::Error::ResponseDeserialize(err__) => {
                        $crate::Error::ResponseDeserialize(err__)
                    }
                    $crate::Error::Io(err__) => $crate::Error::Io(err__),
                }),
            }
        }
    };
}

// allow dead code; we're just testing that the macro expansion compiles
#[allow(dead_code)]
#[cfg(test)]
mod syntax_test {
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
        rpc no_args_ret_error() -> i32;
        rpc one_arg_ret_error(foo: String) -> String;
        rpc no_arg_implicit_return_error();
        #[doc="attr"]
        rpc one_arg_implicit_return_error(foo: String);
    }
}

#[cfg(test)]
mod functional_test {
    use futures::{Future};
    use std::io;
    use std::net::SocketAddr;
    use tokio_core::reactor;
    use util::FirstSocketAddr;
    use future;
    extern crate env_logger;

    macro_rules! unwrap {
        ($e:expr) => {
            match $e {
                Ok(e) => e,
                Err(e) => panic!("{} failed with {:?}", stringify!($e), e),
            }
        };
    }

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
    }

    fn get_future_server_options() -> future::server::Options {
        future::server::Options::default()
    }

    fn get_future_client_options() -> future::client::Options {
        future::client::Options::default()
    }

    fn get_future_client<C>(addr: SocketAddr, handle: reactor::Handle) -> C::ConnectFut
        where C: future::client::ClientExt
    {
        C::connect(addr, get_future_client_options().handle(handle))
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

    mod future_tests {
        use super::{
            env_logger, get_future_client, return_server, start_server_with_async_client,
            FutureClient, FutureService,
        };
        use futures::{finished, Finished};
        use tokio_core::reactor;

        #[derive(Clone)]
        struct Server;

        impl FutureService for Server {
            type AddFut = Finished<i32, ()>;

            fn add(&self, x: i32, y: i32) -> Self::AddFut {
                finished(x + y)
            }

            type HeyFut = Finished<String, ()>;

            fn hey(&self, name: String) -> Self::HeyFut {
                finished(format!("Hey, {}.", name))
            }
        }

        #[test]
        fn simple() {
            let _ = env_logger::try_init();
            let (_, mut reactor, client) = unwrap!(start_server_with_async_client::<
                FutureClient,
                Server,
            >(Server));
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
            let (_, mut reactor, client) = unwrap!(start_server_with_async_client::<
                FutureClient,
                Server,
            >(Server));
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
            use super::FutureServiceExt;
            use future::server;
            use util::FirstSocketAddr;

            let _ = env_logger::try_init();
            let reactor = reactor::Core::new().unwrap();
            let handle = Server
                .listen(
                    "localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default(),
                ).unwrap()
                .0;
            Server
                .listen(handle.addr(), &reactor.handle(), server::Options::default())
                .unwrap();
        }

        #[test]
        fn drop_client() {
            use super::{FutureClient, FutureServiceExt};
            use future::client::ClientExt;
            use future::{client, server};
            use util::FirstSocketAddr;

            let _ = env_logger::try_init();
            let mut reactor = reactor::Core::new().unwrap();
            let (handle, server) = Server
                .listen(
                    "localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default(),
                ).unwrap();
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
    }

    pub mod error_service {
        service! {
            rpc bar() -> Result<u32, ::util::Message>;
        }
    }

    #[derive(Clone)]
    struct ErrorServer;

    impl error_service::FutureService for ErrorServer {
        type BarFut = ::futures::future::FutureResult<Result<u32, ::util::Message>, ()>;

        fn bar(&self) -> Self::BarFut {
            info!("Called bar");
            ::futures::future::ok(Err("lol jk".into()))
        }
    }

    #[test]
    fn error() {
        use self::error_service::*;
        use std::error::Error as E;
        let _ = env_logger::try_init();

        let (_, mut reactor, client) =
            start_err_server_with_async_client::<FutureClient, ErrorServer>(ErrorServer).unwrap();
        reactor
            .run(client.bar().then(move |result| {
                assert_eq!(result.unwrap().err().unwrap().description(), "lol jk");
                Ok::<_, ()>(())
            })).unwrap();
    }

    pub mod other_service {
        service! {
            rpc foo();
        }
    }
}
