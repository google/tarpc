#[doc(hidden)]
#[macro_export]
macro_rules! as_item { ($i:item) => {$i} }

// Required because if-let can't be used with irrefutable patterns, so it needs
// to be special
// cased.
#[doc(hidden)]
#[macro_export]
macro_rules! request_fns {
    ($fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty) => (
        pub fn $fn_name(&self, $($arg: $in_),*) -> $crate::Result<$out> {
            let reply = try!((self.0).rpc(&request_variant!($fn_name $($arg),*)));
            let __Reply::$fn_name(reply) = reply;
            Ok(reply)
        }
    );
    ($( $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty)*) => ( $(
        pub fn $fn_name(&self, $($arg: $in_),*) -> $crate::Result<$out> {
            let reply = try!((self.0).rpc(&request_variant!($fn_name $($arg),*)));
            if let __Reply::$fn_name(reply) = reply {
                Ok(reply)
            } else {
                Err($crate::Error::InternalError)
            }
        }
    )*);
}

// Required because enum variants with no fields can't be suffixed by parens
#[doc(hidden)]
#[macro_export]
macro_rules! define_request {
    ($(@($($finished:tt)*))* --) => (as_item!(
            #[allow(non_camel_case_types)]
            #[derive(Debug, Serialize, Deserialize)]
            enum __Request { $($($finished)*),* }
    ););
    ($(@$finished:tt)* -- $name:ident() $($req:tt)*) =>
        (define_request!($(@$finished)* @($name) -- $($req)*););
    ($(@$finished:tt)* -- $name:ident $args: tt $($req:tt)*) =>
        (define_request!($(@$finished)* @($name $args) -- $($req)*););
    ($($started:tt)*) => (define_request!(-- $($started)*););
}

// Required because enum variants with no fields can't be suffixed by parens
#[doc(hidden)]
#[macro_export]
macro_rules! request_variant {
    ($x:ident) => (__Request::$x);
    ($x:ident $($y:ident),+) => (__Request::$x($($y),+));
}

// The main macro that creates RPC services.
#[macro_export]
macro_rules! rpc {
    (
        mod $server:ident {

            service {
                $(
                    $(#[$attr:meta])*
                    rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty;
                )*
            }
        }
    ) => {
        rpc! {
            mod $server {

                items { }

                service {
                    $(
                        $(#[$attr])*
                        rpc $fn_name($($arg: $in_),*) -> $out;
                    )*
                }
            }
        }
    };

    (
        // Names the service
        mod $server:ident {

            // Include any desired or required items. Conflicts can arise with the following names:
            // 1. Service
            // 2. Client
            // 3. serve
            // 4. __Reply
            // 5. __Request
            items { $($i:item)* }

            // List any rpc methods: rpc foo(arg1: Arg1, ..., argN: ArgN) -> Out
            service {
                $(
                    $(#[$attr:meta])*
                    rpc $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty;
                )*
            }
        }
    ) => {
        #[doc="A module containing an rpc service and client stub."]
        pub mod $server {

            $($i)*

            #[doc="The provided RPC service."]
            pub trait Service: Send + Sync {
                $(
                    $(#[$attr])*
                    fn $fn_name(&self, $($arg:$in_),*) -> $out;
                )*
            }

            define_request!($($fn_name($($in_),*))*);

            #[allow(non_camel_case_types)]
            #[derive(Debug, Serialize, Deserialize)]
            enum __Reply {
                $(
                    $fn_name($out),
                )*
            }

            #[doc="The client stub that makes RPC calls to the server."]
            pub struct Client($crate::protocol::Client<__Request, __Reply>);

            impl Client {
                #[doc="Create a new client that connects to the given address."]
                pub fn new<A>(addr: A, timeout: ::std::option::Option<::std::time::Duration>)
                    -> $crate::Result<Self>
                    where A: ::std::net::ToSocketAddrs,
                {
                    let inner = try!($crate::protocol::Client::new(addr, timeout));
                    Ok(Client(inner))
                }

                request_fns!($($fn_name($($arg: $in_),*) -> $out)*);
            }

            struct __Server<S: 'static + Service>(S);

            impl<S> $crate::protocol::Serve for __Server<S>
                where S: 'static + Service
            {
                type Request = __Request;
                type Reply = __Reply;
                fn serve(&self, request: __Request) -> __Reply {
                    match request {
                        $(
                            request_variant!($fn_name $($arg),*) =>
                                __Reply::$fn_name((self.0).$fn_name($($arg),*)),
                         )*
                    }
                }
            }

            #[doc="Start a running service."]
            pub fn serve<A, S>(addr: A,
                               service: S,
                               read_timeout: ::std::option::Option<::std::time::Duration>)
                -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      S: 'static + Service
            {
                let server = ::std::sync::Arc::new(__Server(service));
                Ok(try!($crate::protocol::serve_async(addr, server, read_timeout)))
            }
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod test {
    use std::time::Duration;

    fn test_timeout() -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    rpc! {
        mod my_server {
            items {
                #[derive(PartialEq, Debug, Serialize, Deserialize)]
                pub struct Foo {
                    pub message: String
                }
            }

            service {
                rpc hello(foo: Foo) -> Foo;
                rpc add(x: i32, y: i32) -> i32;
            }
        }
    }

    use self::my_server::*;

    impl Service for () {
        fn hello(&self, s: Foo) -> Foo {
            Foo { message: format!("Hello, {}", &s.message) }
        }

        fn add(&self, x: i32, y: i32) -> i32 {
            x + y
        }
    }

    #[test]
    fn simple_test() {
        println!("Starting");
        let addr = "127.0.0.1:9000";
        let shutdown = my_server::serve(addr, (), test_timeout()).unwrap();
        let client = Client::new(addr, None).unwrap();
        assert_eq!(3, client.add(1, 2).unwrap());
        let foo = Foo { message: "Adam".into() };
        let want = Foo { message: format!("Hello, {}", &foo.message) };
        assert_eq!(want, client.hello(Foo { message: "Adam".into() }).unwrap());
        drop(client);
        shutdown.shutdown();
    }

    // Tests a service definition with a fn that takes no args
    rpc! {
        mod foo {
            service {
                rpc hello() -> String;
            }
        }
    }

    // Tests a service definition with an import
    rpc! {
        mod bar {
            items {
                use std::collections::HashMap;
            }

            service {
                #[doc="Hello bob"]
                rpc baz(s: String) -> HashMap<String, String>;
            }
        }
    }
}
