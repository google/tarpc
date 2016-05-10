#[macro_use]
extern crate tarpc;
use add::Service as AddService;
use add_one::Service as AddOneService;

mod add {
    service! {
        rpc add(x: i32, y: i32) -> i32;
    }
}

mod add_one {
    service! {
        rpc add_one(x: i32) -> i32;
    }
}

struct AddServer;

impl AddService for AddServer {
    fn add(&mut self, ctx: add::Ctx, x: i32, y: i32) {
        ctx.add(&(x + y)).unwrap();
    }
}

struct AddOneServer(add::Client);

impl AddOneService for AddOneServer {
    fn add_one(&mut self, ctx: add_one::Ctx, x: i32) {
        let ctx = ctx.sendable();
        (self.0)
            .add(|result| ctx.add_one(&result.unwrap()).unwrap(), &x, &1)
            .unwrap();
    }
}

fn main() {
    let server_registry = tarpc::server::Dispatcher::spawn().unwrap();
    let client_registry = tarpc::client::Dispatcher::spawn().unwrap();
    let add = AddServer.register("localhost:0", &server_registry).unwrap();
    let add_client = add::Client::register(add.local_addr(), &client_registry).unwrap();
    let add_one = AddOneServer(add_client);
    let add_one = add_one.register("localhost:0", &server_registry).unwrap();

    let add_one_client = add_one::BlockingClient::register(add_one.local_addr(), &client_registry)
                             .unwrap();
    println!("{}", add_one_client.add_one(&4).unwrap());
}
