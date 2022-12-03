use tarpc::serde_transport;
use tokio_serde::formats::Json;

fn main() {
    #[deny(unused_must_use)]
    {
        serde_transport::tcp::connect::<_, (), (), _, _>("0.0.0.0:0", Json::default);
    }
}
