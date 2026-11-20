//! Jobs that wait on the network run outside any transaction: they can use
//! the store freely (on SQLite a job inside its transaction would wait on
//! its own write lock), and failures are retried like any job's.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group, OutsideJobKind};
use ortus_core::instance::{testing, Instance};
use ortus_core::jobs::{self, NewJob};
use ortus_core::program::AppWorker;
use ortus_core::runtime::block_on;
use ortus_core::storage::Migration;

static TRIES: AtomicUsize = AtomicUsize::new(0);

struct Courier;

const TABLES: &[Migration] = &[Migration {
    version: 1,
    name: "log",
    sql: "CREATE TABLE courier_log (id TEXT PRIMARY KEY)",
}];

impl App for Courier {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "courier",
            group: Group::Communicate,
            path: "/courier",
            order: 1,
        }
    }
    fn migrations(&self) -> &'static [Migration] {
        TABLES
    }
    fn outside_jobs(&self, instance: &Arc<Instance>) -> Vec<OutsideJobKind> {
        let instance = Arc::clone(instance);
        vec![OutsideJobKind {
            kind: "courier.deliver".into(),
            handler: Box::new(move |job| {
                let instance = Arc::clone(&instance);
                Box::pin(async move {
                    // The first try fails, as a busy remote server would.
                    if TRIES.fetch_add(1, Ordering::SeqCst) == 0 {
                        return Err("421 try later".into());
                    }
                    instance
                        .store
                        .execute(
                            "INSERT INTO courier_log (id) VALUES ($1)",
                            &[job.id.as_str().into()],
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                })
            }),
        }]
    }
}

#[test]
fn outside_jobs_use_the_store_and_are_retried() {
    let instance = testing::instance(vec![Arc::new(Courier)]);
    let worker = AppWorker::new(Arc::clone(&instance), "test");
    block_on(jobs::enqueue(
        instance.store.as_ref(),
        &NewJob {
            id: "parcel-1",
            kind: "courier.deliver",
            payload: "",
            run_at: 0,
            max_attempts: 5,
        },
        0,
    ))
    .unwrap();
    let started = std::time::Instant::now();
    while block_on(worker.tick(1_000)).unwrap() {}
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "no waiting on its own lock"
    );
    assert_eq!(1, TRIES.load(Ordering::SeqCst));
    // Later, the retry goes through and the job is done.
    while block_on(worker.tick(1_000_000)).unwrap() {}
    assert_eq!(2, TRIES.load(Ordering::SeqCst));
    let logged = block_on(instance.store.query("SELECT id FROM courier_log", &[])).unwrap();
    assert_eq!(Some("parcel-1"), logged[0].text(0));
    assert_eq!(
        Some(jobs::JobState::Done),
        block_on(jobs::state(instance.store.as_ref(), "parcel-1")).unwrap()
    );
}
