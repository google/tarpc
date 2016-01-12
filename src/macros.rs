#[macro_export]
macro_rules! as_item { ($i:item) => {$i} }

// Required because if-let can't be used with irrefutable patterns, so it needs to be special
// cased.
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
#[macro_export]
macro_rules! request_variant {
    ($x:ident) => (__Request::$x);
    ($x:ident $($y:ident),+) => (__Request::$x($($y),+));
}

// The main macro that creates RPC services.
#[macro_export]
macro_rules! rpc_service { ($server:ident: 
    $( $fn_name:ident( $( $arg:ident : $in_:ty ),* ) -> $out:ty;)*) => {
        #[doc="A module containing an rpc service and client stub."]
        pub mod $server {

            #[doc="The provided RPC service."]
            pub trait Service: Send + Sync {
                $(
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
                pub fn new<A>(addr: A) -> $crate::Result<Self>
                    where A: ::std::net::ToSocketAddrs,
                {
                    let inner = try!($crate::protocol::Client::new(addr));
                    Ok(Client(inner))
                }

                request_fns!($($fn_name($($arg: $in_),*) -> $out)*);
            }

            struct __Server<S: 'static + Service>(S);

            impl<S> $crate::protocol::Serve<__Request, __Reply> for __Server<S>
                where S: 'static + Service
            {
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
            pub fn serve<A, S>(addr: A, service: S) -> $crate::Result<$crate::protocol::ServeHandle>
                where A: ::std::net::ToSocketAddrs,
                      S: 'static + Service
            {
                let server = ::std::sync::Arc::new(__Server(service));
                Ok(try!($crate::protocol::serve_async(addr, server)))
            }
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod test {
    rpc_service!(my_server:
        hello(foo: super::Foo) -> super::Foo;

        add(x: i32, y: i32) -> i32;
    );

    use self::my_server::*;

    #[derive(PartialEq, Debug, Serialize, Deserialize)]
    pub struct Foo {
        message: String
    }

    impl Service for () {
        fn hello(&self, s: Foo) -> Foo {
            Foo{message: format!("Hello, {}", &s.message)}
        }

        fn add(&self, x: i32, y: i32) -> i32 {
            x + y
        }
    }

    #[test]
    fn simple_test() {
        println!("Starting");
        let addr = "127.0.0.1:9000";
        let shutdown = my_server::serve(addr, ()).unwrap();
        let client = Client::new(addr).unwrap();
        assert_eq!(3, client.add(1, 2).unwrap());
        let foo = Foo{message: "Adam".into()};
        let want = Foo{message: format!("Hello, {}", &foo.message)};
        assert_eq!(want, client.hello(Foo{message: "Adam".into()}).unwrap());
        drop(client);
        shutdown.shutdown();
    }

    // This is a test of a service with a fn that takes no args
    rpc_service! {foo:
        hello() -> String;
    }
}
