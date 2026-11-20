//! Starting and running an Ortus program from its configuration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rust_web_server::prelude::*;

use crate::app::App;
use crate::config::{Config, Role};
use crate::instance::Instance;
use crate::jobs::Worker;

/// The time now, in Unix milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Runs the program described by `config`, with `apps`, until it is told
/// to stop.
///
/// Every program opens the database and brings every schema up to date. A
/// program then does only what its roles say: the web role serves HTTP,
/// the worker role runs background jobs, and each app runs its tasks for
/// the roles it has (mail listens for SMTP in the mail role). A program
/// without the web role opens no HTTP listener.
pub async fn run(config: Config, apps: Vec<Arc<dyn App>>) -> Result<(), String> {
    // Listen for the stop signal before anything else, so a stop that
    // comes while the program is still starting is not lost.
    let stop = stop_signal()?;
    run_until(config, apps, async move {
        if let Err(e) = stop.await {
            eprintln!("ortus: {e}");
        }
    })
    .await
}

/// As [`run`], stopping when `stop` resolves instead of on a signal (for
/// tests and embedding).
pub async fn run_until(
    config: Config,
    apps: Vec<Arc<dyn App>>,
    stop: impl std::future::Future<Output = ()> + Send,
) -> Result<(), String> {
    let roles: Vec<&str> = config.roles.iter().map(|role| role.name()).collect();
    println!("ortus: roles: {}", roles.join(", "));
    let ids: Vec<&str> = apps.iter().map(|app| app.info().id).collect();
    println!(
        "ortus: apps: {}",
        if ids.is_empty() {
            "none".to_string()
        } else {
            ids.join(", ")
        }
    );

    let instance = Instance::open(config, apps, now_ms()).await?;
    let config = &instance.config;

    let stopping = Arc::new(AtomicBool::new(false));
    let worker = config.has(Role::Worker).then(|| {
        println!("ortus: worker: taking background jobs");
        tokio::spawn(run_worker(Arc::clone(&instance), Arc::clone(&stopping)))
    });

    // Each app's tasks for the roles this program has, if the app is on.
    let (stop_tasks, task_stop) = tokio::sync::watch::channel(false);
    let mut tasks = Vec::new();
    for app in &instance.apps {
        let id = app.info().id;
        let on = instance
            .switches
            .is_on(id)
            .await
            .map_err(|e| e.to_string())?;
        for task in app.role_tasks() {
            if !config.has(task.role) {
                continue;
            }
            if !on {
                println!("ortus: {id}: off; not starting {}", task.name);
                continue;
            }
            println!("ortus: {id}: {} ({})", task.name, task.role.name());
            let (run, name) = (task.run, task.name);
            let future = run(Arc::clone(&instance), task_stop.clone());
            tasks.push(tokio::spawn(async move {
                if let Err(e) = future.await {
                    eprintln!("ortus: {id}: {name} stopped: {e}");
                }
            }));
        }
    }

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
        // The listener is bound: tell systemd (Type=notify) we are up.
        #[cfg(unix)]
        rust_web_server::systemd::ready("serving");
        // On SIGTERM rws stops on its own and drains; `stop` is for the
        // other ways of stopping.
        tokio::select! {
            () = Server::run_tls(listener, pool, crate::app(Arc::clone(&instance))) => {}
            () = stop => {}
        }
    } else {
        println!("ortus: no web role: not listening for HTTP");
        #[cfg(unix)]
        rust_web_server::systemd::ready("running");
        stop.await;
    }
    println!("ortus: stopping");

    stopping.store(true, Ordering::SeqCst);
    let _ = stop_tasks.send(true);
    for task in tasks {
        let _ = task.await;
    }
    if let Some(worker) = worker {
        let _ = worker.await;
    }
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM (how systemd and Kubernetes stop a
/// program). The handlers are in place when this returns.
fn stop_signal() -> Result<impl std::future::Future<Output = Result<(), String>>, String> {
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    let interrupt = tokio::signal::ctrl_c();
    Ok(async move {
        #[cfg(unix)]
        tokio::select! {
            result = interrupt => result.map_err(|e| e.to_string()),
            _ = term.recv() => Ok(()),
        }
        #[cfg(not(unix))]
        interrupt.await.map_err(|e| e.to_string())
    })
}

/// The worker of an installation: every app's kinds of job, run only
/// while their app is on.
pub struct AppWorker {
    instance: Arc<Instance>,
    worker: Worker,
    owners: Vec<(String, &'static str)>,
}

impl AppWorker {
    pub fn new(instance: Arc<Instance>, name: &str) -> Self {
        let mut worker = Worker::new(name);
        let mut owners = Vec::new();
        for app in &instance.apps {
            let id = app.info().id;
            for job in app.jobs(&instance) {
                if !job.kind.starts_with(&format!("{id}.")) {
                    eprintln!(
                        "ortus: {id}: job kind {} does not start with '{id}.'; not run",
                        job.kind
                    );
                    continue;
                }
                owners.push((job.kind.clone(), id));
                worker = worker.handle(&job.kind, job.handler);
            }
        }
        Self {
            instance,
            worker,
            owners,
        }
    }

    /// Runs at most one due job. Jobs of a switched-off app stay in the
    /// queue until it is on again.
    pub async fn tick(&self, now_ms: i64) -> Result<bool, String> {
        let mut kinds = Vec::new();
        for (kind, id) in &self.owners {
            if self
                .instance
                .switches
                .is_on(id)
                .await
                .map_err(|e| e.to_string())?
            {
                kinds.push(kind.as_str());
            }
        }
        self.worker
            .tick_only(self.instance.store.as_ref(), now_ms, &kinds)
            .await
            .map_err(|e| e.to_string())
    }
}

async fn run_worker(instance: Arc<Instance>, stopping: Arc<AtomicBool>) {
    // Unique among every worker on the database: in the chart every pod's
    // program is process 1, so the pod name and the start time go in too.
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "host".into());
    let name = format!("worker-{host}-{}-{}", std::process::id(), now_ms());
    let worker = AppWorker::new(instance, &name);
    let idle = Duration::from_millis(500);
    while !stopping.load(Ordering::SeqCst) {
        match worker.tick(now_ms()).await {
            Ok(true) => {}
            Ok(false) | Err(_) => tokio::time::sleep(idle).await,
        }
    }
}
