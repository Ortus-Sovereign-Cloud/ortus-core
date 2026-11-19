//! Runs the core on its own, for development. The released programs are
//! built in the `ortus` repository.

use rust_web_server::prelude::*;

#[tokio::main]
async fn main() {
    let (listener, pool) = Server::setup().expect("ortus-core: could not start the server");
    Server::run_tls(listener, pool, ortus_core::app()).await;
}
