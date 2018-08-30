// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

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
/// * `Service` -- the trait defining the RPC service via a `Future` API.
/// * `ServiceExt` -- provides the methods for starting a service. There is an umbrella impl
///                         for all implers of `Service`. It's a separate trait to prevent
///                         name collisions with RPCs.
/// * `Client` -- a client whose RPCs return `Future`s.
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
// Pattern for when the next rpc has an implicit unit return type.
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
// Pattern for when the next rpc has an explicit return type.
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

        #[derive(Debug)]
        #[doc(hidden)]
        #[allow(non_camel_case_types, unused)]
        #[derive($crate::serde_derive::Serialize, $crate::serde_derive::Deserialize)]
        pub enum Request__ {
            $(
                $fn_name{ $($arg: $in_,)* }
            ),*
        }

        #[derive(Debug)]
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
        pub trait Service: Clone + Send + 'static {
            $(
                snake_to_camel! {
                    /// The type of future returned by `{}`.
                    type $fn_name: $crate::futures::Future<Output = $out> + Send;
                }

                $(#[$attr])*
                fn $fn_name(&self, ctx: &$crate::rpc::server::Context, $($arg:$in_),*) -> ty_snake_to_camel!(Self::$fn_name);
            )*
        }

        existential type Resp<S>: ::std::future::Future<Output=::std::io::Result<Response__>> + Send;

        /// Returns a serving function to use with rpc::server::Server.
        pub fn serve<S: Service>(mut service: S)
            -> impl FnMut(&$crate::rpc::server::Context, Request__) -> Resp<S> + Send + 'static + Clone {
                move |ctx, req| {
                    match req {
                        $(
                            Request__::$fn_name{ $($arg,)* } => {
                                let resp = Service::$fn_name(&mut service, ctx, $($arg),*);
                                async {
                                    let resp = await!(resp);
                                    Ok(Response__::$fn_name(resp))
                                }
                            }
                        )*
                    }
                }
            }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct Client($crate::rpc::client::Client<Request__, Response__>);

        /// Returns a new client stub that sends requests over the given transport.
        pub async fn newStub<T>(config: $crate::rpc::client::Config, transport: T) -> Client
        where
            T: $crate::rpc::Transport<
                    Item = $crate::rpc::Response<Response__>,
                    SinkItem = $crate::rpc::ClientMessage<Request__>> + Send,
        {
            Client(await!($crate::rpc::client::Client::new(config, transport)))
        }

        impl Client {
            $(
                #[allow(unused)]
                $(#[$attr])*
                pub fn $fn_name(&mut self, ctx: $crate::rpc::client::Context, $($arg: $in_),*)
                    -> impl ::std::future::Future<Output = ::std::io::Result<$out>> + '_ {
                    let request__ = Request__::$fn_name { $($arg,)* };
                    let resp = self.0.send(ctx, request__);
                    async move {
                        match await!(resp)? {
                            Response__::$fn_name(msg__) => ::std::result::Result::Ok(msg__),
                            _ => unreachable!(),
                        }
                    }
                }
            )*
        }
    }
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
    use crate::future;
    use crate::util::FirstSocketAddr;
    use futures::Future;
    use std::io;
    use std::net::SocketAddr;
    use tokio_core::reactor;
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
    where
        C: future::client::ClientExt,
    {
        C::connect(addr, get_future_client_options().handle(handle))
    }

    fn start_server_with_async_client<C, S>(
        server: S,
    ) -> io::Result<(future::server::Handle, reactor::Core, C)>
    where
        C: future::client::ClientExt,
        S: ServiceExt,
    {
        let mut reactor = reactor::Core::new()?;
        let options = get_future_server_options();
        let (handle, server) = server.listen(
            "localhost:0".first_socket_addr(),
            &reactor.handle(),
            options,
        )?;
        reactor.handle().spawn(server);
        let client = unwrap!(reactor.run(C::connect(handle.addr(), get_future_client_options())));
        Ok((handle, reactor, client))
    }

    fn return_server<S>(server: S) -> io::Result<(future::server::Handle, reactor::Core, Listen<S>)>
    where
        S: ServiceExt,
    {
        let reactor = reactor::Core::new()?;
        let options = get_future_server_options();
        let (handle, server) = server.listen(
            "localhost:0".first_socket_addr(),
            &reactor.handle(),
            options,
        )?;
        Ok((handle, reactor, server))
    }

    fn start_err_server_with_async_client<C, S>(
        server: S,
    ) -> io::Result<(future::server::Handle, reactor::Core, C)>
    where
        C: future::client::ClientExt,
        S: error_service::ServiceExt,
    {
        let mut reactor = reactor::Core::new()?;
        let options = get_future_server_options();
        let (handle, server) = server.listen(
            "localhost:0".first_socket_addr(),
            &reactor.handle(),
            options,
        )?;
        reactor.handle().spawn(server);
        let client = C::connect(handle.addr(), get_future_client_options());
        let client = unwrap!(reactor.run(client));
        Ok((handle, reactor, client))
    }

    mod future_tests {
        use super::{
            env_logger, get_future_client, return_server, start_server_with_async_client,
            Client, Service,
        };
        use futures::{finished, Finished};
        use tokio_core::reactor;

        #[derive(Clone)]
        struct Server;

        impl Service for Server {
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
                Client,
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
            use tokio_core::reactor;

            let _ = env_logger::try_init();
            let (handle, mut reactor, server) = unwrap!(return_server::<Server>(Server));

            let (tx, rx) = ::std::sync::mpsc::channel();
            ::std::thread::spawn(move || {
                let mut reactor = reactor::Core::new().unwrap();
                let client = get_future_client::<Client>(handle.addr(), reactor.handle());
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
                Client,
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
                super::other_service::Client,
                Server,
            >(Server));
            match reactor.run(client.foo()).err().unwrap() {
                crate::Error::RequestDeserialize(_) => {} // good
                bad => panic!(r#"Expected Error::RequestDeserialize but got "{}""#, bad),
            }
        }

        #[test]
        fn reuse_addr() {
            use super::ServiceExt;
            use crate::future::server;
            use crate::util::FirstSocketAddr;

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
            use super::{Client, ServiceExt};
            use crate::future::client::ClientExt;
            use crate::future::{client, server};
            use crate::util::FirstSocketAddr;

            let _ = env_logger::try_init();
            let mut reactor = reactor::Core::new().unwrap();
            let (handle, server) = Server
                .listen(
                    "localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default(),
                ).unwrap();
            reactor.handle().spawn(server);

            let client = Client::connect(
                handle.addr(),
                client::Options::default().handle(reactor.handle()),
            );
            let client = unwrap!(reactor.run(client));
            assert_eq!(reactor.run(client.add(1, 2)).unwrap(), 3);
            drop(client);

            let client = Client::connect(
                handle.addr(),
                client::Options::default().handle(reactor.handle()),
            );
            let client = unwrap!(reactor.run(client));
            assert_eq!(reactor.run(client.add(1, 2)).unwrap(), 3);
        }
    }

    pub mod error_service {
        service! {
            rpc bar() -> Result<u32, crate::util::Message>;
        }
    }

    #[derive(Clone)]
    struct ErrorServer;

    impl error_service::Service for ErrorServer {
        type BarFut = ::futures::future::FutureResult<Result<u32, crate::util::Message>, ()>;

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
            start_err_server_with_async_client::<Client, ErrorServer>(ErrorServer).unwrap();
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
