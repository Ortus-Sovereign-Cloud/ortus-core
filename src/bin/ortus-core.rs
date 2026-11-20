//! Runs the core on its own, for development and tests. The released
//! programs are built in the `ortus` repository and use the same command
//! line.

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = ortus_core::cli::run(
        "ortus-core",
        ortus_core::VERSION,
        &ortus_core::schemas(),
        &args,
    )
    .await;
    std::process::exit(status);
}
