//! Runs the core on its own, for development. The released programs are
//! built in the `ortus` repository.

use ortus_core::config::Config;

#[tokio::main]
async fn main() {
    let config = match Config::from_process() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("ortus: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = ortus_core::program::run(config).await {
        eprintln!("ortus: {e}");
        std::process::exit(1);
    }
}
