// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#[cfg(feature = "serde")]
#[doc(hidden)]
#[macro_export]
macro_rules! add_serde_if_enabled {
    ($(#[$attr:meta])* -- $i:item) => {
        $(#[$attr])*
        #[derive($crate::serde::Serialize, $crate::serde::Deserialize)]
        $i
    }
}

#[cfg(not(feature = "serde"))]
#[doc(hidden)]
#[macro_export]
macro_rules! add_serde_if_enabled {
    ($(#[$attr:meta])* -- $i:item) => {
        $(#[$attr])*
        $i
    }
}

/// The main macro that creates RPC services.
///
/// Rpc methods are specified, mirroring trait syntax:
///
/// ```
/// # #![feature(arbitrary_self_types, async_await, proc_macro_hygiene)]
/// # fn main() {}
/// # tarpc::service! {
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
/// * `trait Service` -- defines the RPC service.
///   * `fn serve` -- turns a service impl into a request handler.
/// * `Client` -- a client stub with a fn for each RPC.
///   * `fn new_stub` -- creates a new Client stub.
///
#[macro_export]
macro_rules! service {
    () => {
        compile_error!("Must define at least one RPC method.");
    };
// Entry point
    (
        $(
            $(#[$attr:meta])*
            rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) $(-> $out:ty)*;
        )*
    ) => {
        $crate::service! {{
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
        $crate::service! {
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
        $crate::service! {
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
        $crate::add_serde_if_enabled! {
            /// The request sent over the wire from the client to the server.
            #[derive(Debug)]
            #[allow(non_camel_case_types, unused)]
            --
            pub enum Request {
                $(
                    $(#[$attr])*
                    $fn_name{ $($arg: $in_,)* }
                ),*
            }
        }

        $crate::add_serde_if_enabled! {
            /// The response sent over the wire from the server to the client.
            #[derive(Debug)]
            #[allow(non_camel_case_types, unused)]
            --
            pub enum Response {
                $(
                    $(#[$attr])*
                    $fn_name($out)
                ),*
            }
        }

        // TODO: proc_macro can't currently parse $crate, so this needs to be imported for the
        // usage of snake_to_camel! to work.
        use $crate::futures::Future as Future__;

        /// Defines the RPC service. The additional trait bounds are required so that services can
        /// multiplex requests across multiple tasks, potentially on multiple threads.
        pub trait Service: Clone + Send + 'static {
            $(
                $crate::snake_to_camel! {
                    /// The type of future returned by `{}`.
                    type $fn_name: Future__<Output = $out> + Send;
                }

                $(#[$attr])*
                fn $fn_name(self, ctx: $crate::context::Context, $($arg:$in_),*) -> $crate::ty_snake_to_camel!(Self::$fn_name);
            )*
        }

        // TODO: use an existential type instead of this when existential types work.
        /// A future resolving to a server [`Response`].
        #[allow(non_camel_case_types)]
        pub enum ResponseFut<S: Service> {
            $(
                $(#[$attr])*
                $fn_name($crate::ty_snake_to_camel!(<S as Service>::$fn_name)),
            )*
        }

        impl<S: Service> ::std::fmt::Debug for ResponseFut<S> {
            fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                fmt.debug_struct("Response").finish()
            }
        }

        impl<S: Service> ::std::future::Future for ResponseFut<S> {
            type Output = ::std::io::Result<Response>;

            fn poll(self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                -> ::std::task::Poll<::std::io::Result<Response>>
            {
                unsafe {
                    match ::std::pin::Pin::get_unchecked_mut(self) {
                        $(
                            ResponseFut::$fn_name(resp) =>
                                ::std::pin::Pin::new_unchecked(resp)
                                    .poll(cx)
                                    .map(Response::$fn_name)
                                    .map(Ok),
                        )*
                    }
                }
            }
        }

        /// Returns a serving function to use with rpc::server::Server.
        pub fn serve<S: Service>(service: S)
            -> impl FnOnce($crate::context::Context, Request) -> ResponseFut<S> + Send + 'static + Clone {
                move |ctx, req| {
                    match req {
                        $(
                            Request::$fn_name{ $($arg,)* } => {
                                let resp = Service::$fn_name(service.clone(), ctx, $($arg),*);
                                ResponseFut::$fn_name(resp)
                            }
                        )*
                    }
                }
            }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        pub struct Client<C = $crate::client::Channel<Request, Response>>(C);

        /// Returns a new client stub that sends requests over the given transport.
        pub async fn new_stub<T>(config: $crate::client::Config, transport: T)
            -> ::std::io::Result<Client>
        where
            T: $crate::Transport<$crate::ClientMessage<Request>, $crate::Response<Response>> + Send + 'static,
        {
            Ok(Client($crate::client::new(config, transport).await?))
        }

        impl<C> From<C> for Client<C>
            where for <'a> C: $crate::Client<'a, Request, Response = Response>
        {
            fn from(client: C) -> Self {
                Client(client)
            }
        }

        impl<C> Client<C>
            where for<'a> C: $crate::Client<'a, Request, Response = Response>
        {
            $(
                #[allow(unused)]
                $(#[$attr])*
                pub fn $fn_name(&mut self, ctx: $crate::context::Context, $($arg: $in_),*)
                    -> impl ::std::future::Future<Output = ::std::io::Result<$out>> + '_ {
                    let request__ = Request::$fn_name { $($arg,)* };
                    let resp = $crate::Client::call(&mut self.0, ctx, request__);
                    async move {
                        match resp.await? {
                            Response::$fn_name(msg__) => ::std::result::Result::Ok(msg__),
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
    use futures::{
        compat::Executor01CompatExt,
        future::{ready, Ready},
        prelude::*,
    };
    use rpc::{client, context, server::Handler, transport::channel};
    use std::io;
    use tokio::runtime::current_thread;

    service! {
        rpc add(x: i32, y: i32) -> i32;
        rpc hey(name: String) -> String;
    }

    #[derive(Clone)]
    struct Server;

    impl Service for Server {
        type AddFut = Ready<i32>;

        fn add(self, _: context::Context, x: i32, y: i32) -> Self::AddFut {
            ready(x + y)
        }

        type HeyFut = Ready<String>;

        fn hey(self, _: context::Context, name: String) -> Self::HeyFut {
            ready(format!("Hey, {}.", name))
        }
    }

    #[test]
    fn sequential() {
        let _ = env_logger::try_init();
        rpc::init(tokio::executor::DefaultExecutor::current().compat());

        let test = async {
            let (tx, rx) = channel::unbounded();
            tokio_executor::spawn(
                crate::Server::default()
                    .incoming(stream::once(ready(rx)))
                    .respond_with(serve(Server))
                    .unit_error()
                    .boxed()
                    .compat(),
            );

            let mut client = new_stub(client::Config::default(), tx).await?;
            assert_eq!(3, client.add(context::current(), 1, 2).await?);
            assert_eq!(
                "Hey, Tim.",
                client.hey(context::current(), "Tim".to_string()).await?
            );
            Ok::<_, io::Error>(())
        }
            .map_err(|e| panic!(e.to_string()));

        current_thread::block_on_all(test.boxed().compat()).unwrap();
    }

    #[test]
    fn concurrent() {
        let _ = env_logger::try_init();
        rpc::init(tokio::executor::DefaultExecutor::current().compat());

        let test = async {
            let (tx, rx) = channel::unbounded();
            tokio_executor::spawn(
                rpc::Server::default()
                    .incoming(stream::once(ready(rx)))
                    .respond_with(serve(Server))
                    .unit_error()
                    .boxed()
                    .compat(),
            );

            let client = new_stub(client::Config::default(), tx).await?;
            let mut c = client.clone();
            let req1 = c.add(context::current(), 1, 2);
            let mut c = client.clone();
            let req2 = c.add(context::current(), 3, 4);
            let mut c = client.clone();
            let req3 = c.hey(context::current(), "Tim".to_string());

            assert_eq!(3, req1.await?);
            assert_eq!(7, req2.await?);
            assert_eq!("Hey, Tim.", req3.await?);
            Ok::<_, io::Error>(())
        }
            .map_err(|e| panic!("test failed: {}", e));

        current_thread::block_on_all(test.boxed().compat()).unwrap();
    }
}
