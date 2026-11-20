//! An app's periodic jobs run on schedule, once per period across workers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group, JobKind};
use ortus_core::instance::{testing, Instance};
use ortus_core::program::AppWorker;
use ortus_core::runtime::block_on;

static RUNS: AtomicUsize = AtomicUsize::new(0);

struct Ticker;

impl App for Ticker {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "ticker",
            group: Group::Create,
            path: "/ticker",
            order: 1,
        }
    }
    fn jobs(&self, _: &Arc<Instance>) -> Vec<JobKind> {
        vec![JobKind {
            kind: "ticker.check".into(),
            handler: Box::new(|_, _| {
                Box::pin(async {
                    RUNS.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        }]
    }
    fn periodic(&self) -> Vec<(&'static str, i64)> {
        // Too often, and a kind it does not handle: both ignored.
        vec![
            ("ticker.check", 10 * 60_000),
            ("ticker.fast", 1_000),
            ("ticker.unknown", 60_000),
        ]
    }
}

#[test]
fn an_apps_periodic_job_runs_once_per_period() {
    let instance = testing::instance(vec![Arc::new(Ticker)]);
    let first = AppWorker::new(Arc::clone(&instance), "a");
    let second = AppWorker::new(Arc::clone(&instance), "b");
    let at = 1_800_000_000_000;
    for worker in [&first, &second] {
        while block_on(worker.tick(at)).unwrap() {}
    }
    assert_eq!(
        1,
        RUNS.load(Ordering::SeqCst),
        "one run for the period, whatever the number of workers"
    );
    while block_on(first.tick(at + 5 * 60_000)).unwrap() {}
    assert_eq!(
        1,
        RUNS.load(Ordering::SeqCst),
        "not again within the period"
    );
    while block_on(second.tick(at + 11 * 60_000)).unwrap() {}
    assert_eq!(2, RUNS.load(Ordering::SeqCst), "the next period");
}
