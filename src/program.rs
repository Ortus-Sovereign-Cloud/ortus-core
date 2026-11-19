//! Starting and running an Ortus program from its configuration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rust_web_server::prelude::*;

use crate::config::{Config, Role};
use crate::jobs::{self, Worker};
use crate::storage::AsyncStore;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Runs the program described by `config` until it is told to stop.
///
/// Every program opens the database and brings its schema up to date. A
/// program then does only what its roles say: the web role serves HTTP,
/// the worker role runs background jobs. A program without the web role
/// opens no HTTP listener.
pub async fn run(config: Config) -> Result<(), String> {
    let roles: Vec<&str> = config.roles.iter().map(|role| role.name()).collect();
    println!("ortus: roles: {}", roles.join(", "));

    let store = config.open_store().await.map_err(|e| e.to_string())?;
    jobs::install(store.as_ref(), now_ms())
        .await
        .map_err(|e| e.to_string())?;
    config.open_files().map_err(|e| e.to_string())?;

    let stopping = Arc::new(AtomicBool::new(false));
    let worker = config.has(Role::Worker).then(|| {
        println!("ortus: worker: taking background jobs");
        tokio::spawn(run_worker(Arc::clone(&store), Arc::clone(&stopping)))
    });

    for listener in config.listeners() {
        println!(
            "ortus: {}: {}://{}:{}",
            listener.role.name(),
            listener.protocol,
            listener.ip,
            listener.port
        );
    }
    if config.has(Role::Web) {
        // rws reads its address from its own settings; ours take
        // precedence so one configuration describes the whole program.
        std::env::set_var("RWS_CONFIG_IP", &config.web_ip);
        std::env::set_var("RWS_CONFIG_PORT", config.web_port.to_string());
        let (listener, pool) = Server::setup()?;
        Server::run_tls(listener, pool, crate::app()).await;
    } else {
        println!("ortus: no web role: not listening for HTTP");
        tokio::signal::ctrl_c().await.map_err(|e| e.to_string())?;
    }

    stopping.store(true, Ordering::SeqCst);
    if let Some(worker) = worker {
        let _ = worker.await;
    }
    Ok(())
}

async fn run_worker(store: Arc<dyn AsyncStore>, stopping: Arc<AtomicBool>) {
    // No handlers yet: each app registers its own when it is added.
    let worker = Worker::new(&format!("worker-{}", std::process::id()));
    worker
        .run(store.as_ref(), Duration::from_millis(500), &|| {
            stopping.load(Ordering::SeqCst)
        })
        .await;
}
