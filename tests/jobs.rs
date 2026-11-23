//! The job queue, on every database engine.

mod common;

use std::sync::Arc;

use common::{postgres, sqlite, Fresh};
use ortus_core::jobs::blocking::{
    claim, complete, enqueue, enqueue_in, fail, install, state, tick,
};
use ortus_core::jobs::{JobState, NewJob, Worker};
use ortus_core::storage::blocking::on;
use ortus_core::storage::{AsyncStore, StoreError, Value};

macro_rules! on_both_engines {
    ($($name:ident),* $(,)?) => {
        mod on_sqlite { $( #[test] fn $name() { if let Some(db) = super::sqlite() { super::$name(&db) } } )* }
        mod on_postgres { $( #[test] fn $name() { if let Some(db) = super::postgres() { super::$name(&db) } } )* }
    };
}

on_both_engines!(
    a_job_survives_a_crash_and_runs_exactly_once,
    a_job_is_not_run_before_its_time,
    the_same_id_is_one_job,
    a_failing_job_is_retried_then_marked_failed,
    a_failed_handler_leaves_no_writes_behind,
    many_workers_run_every_job_once,
    a_job_enqueued_in_a_rolled_back_transaction_does_not_exist,
    jobs_of_other_kinds_never_hide_a_workers_jobs,
    a_slow_job_keeps_its_lease,
    finished_jobs_are_forgotten,
);

fn job<'a>(id: &'a str, kind: &'a str) -> NewJob<'a> {
    NewJob {
        id,
        kind,
        payload: "{}",
        run_at: 0,
        max_attempts: 3,
    }
}

/// A database with the job table and a table that counts handler runs.
fn open(db: &Fresh) -> Arc<dyn AsyncStore> {
    let store = (db.open)();
    install(store.as_ref(), 0).unwrap();
    on(&*store)
        .execute(
            "CREATE TABLE IF NOT EXISTS runs (job TEXT PRIMARY KEY, times BIGINT NOT NULL)",
            &[],
        )
        .unwrap();
    store
}

/// A worker whose handler records one run of the job in `runs`.
fn counting_worker(name: &str) -> Worker {
    Worker::new(name)
        .lease_ms(1_000)
        .retry_delay_ms(100)
        .handle(
            "count",
            Box::new(|job, tx| {
                Box::pin(async move {
                    let id = [Value::from(job.id.as_str())];
                    let seen = tx
                        .query("SELECT times FROM runs WHERE job = $1", &id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let written = match seen.first().and_then(|row| row.int(0)) {
                        Some(times) => {
                            tx.execute(
                                "UPDATE runs SET times = $1 WHERE job = $2",
                                &[(times + 1).into(), job.id.as_str().into()],
                            )
                            .await
                        }
                        None => {
                            tx.execute("INSERT INTO runs (job, times) VALUES ($1, 1)", &id)
                                .await
                        }
                    };
                    written.map(|_| ()).map_err(|e| e.to_string())
                })
            }),
        )
}

fn runs(store: &dyn AsyncStore) -> Vec<(String, i64)> {
    on(store)
        .query("SELECT job, times FROM runs ORDER BY job", &[])
        .unwrap()
        .iter()
        .map(|row| (row.text(0).unwrap().to_string(), row.int(1).unwrap()))
        .collect()
}

fn a_job_survives_a_crash_and_runs_exactly_once(db: &Fresh) {
    let store = open(db);
    assert!(enqueue(store.as_ref(), &job("j1", "count"), 0).unwrap());

    // A worker takes the job and its program dies before finishing: it
    // never completes or fails the job, and its connection goes away.
    let crashed = claim(store.as_ref(), "worker-a", &[], 10, 1_000)
        .unwrap()
        .expect("the job is due");
    assert_eq!(
        Some(JobState::Running),
        state(store.as_ref(), "j1").unwrap()
    );
    drop(store);

    // A new program starts on the same database. While the dead worker's
    // lease lasts the job is left alone; after it, the job runs.
    let store = (db.open)();
    let worker = counting_worker("worker-b");
    assert!(
        !tick(&worker, store.as_ref(), 500).unwrap(),
        "the lease has not run out yet"
    );
    assert!(runs(store.as_ref()).is_empty());
    assert!(tick(&worker, store.as_ref(), 1_010).unwrap());
    assert_eq!(Some(JobState::Done), state(store.as_ref(), "j1").unwrap());
    assert_eq!(vec![("j1".to_string(), 1)], runs(store.as_ref()));

    // Nothing is left to run, however often workers look.
    assert!(!tick(&worker, store.as_ref(), 5_000).unwrap());
    assert!(!tick(&counting_worker("worker-c"), store.as_ref(), 50_000).unwrap());

    // The dead worker turns out to be only slow and finishes late: its
    // result is refused and its writes are not kept.
    let late = complete(store.as_ref(), &crashed, "worker-a", 60_000, &mut |tx| {
        tx.execute("UPDATE runs SET times = times + 1 WHERE job = 'j1'", &[])
            .map(|_| ())
    })
    .unwrap();
    assert!(!late);
    assert_eq!(vec![("j1".to_string(), 1)], runs(store.as_ref()));
}

fn a_job_is_not_run_before_its_time(db: &Fresh) {
    let store = open(db);
    enqueue(
        store.as_ref(),
        &NewJob {
            run_at: 1_000,
            ..job("later", "count")
        },
        0,
    )
    .unwrap();
    let worker = counting_worker("w");
    assert!(!tick(&worker, store.as_ref(), 999).unwrap());
    assert!(tick(&worker, store.as_ref(), 1_000).unwrap());
    assert_eq!(vec![("later".to_string(), 1)], runs(store.as_ref()));
}

fn the_same_id_is_one_job(db: &Fresh) {
    let store = open(db);
    assert!(enqueue(store.as_ref(), &job("same", "count"), 0).unwrap());
    assert!(!enqueue(store.as_ref(), &job("same", "count"), 1).unwrap());
    let worker = counting_worker("w");
    assert!(tick(&worker, store.as_ref(), 10).unwrap());
    assert!(!tick(&worker, store.as_ref(), 20).unwrap());
    // Adding it again after it ran still does not run it twice.
    assert!(!enqueue(store.as_ref(), &job("same", "count"), 30).unwrap());
    assert!(!tick(&worker, store.as_ref(), 40).unwrap());
    assert_eq!(vec![("same".to_string(), 1)], runs(store.as_ref()));
    assert_eq!(None, state(store.as_ref(), "unknown").unwrap());
}

fn a_failing_job_is_retried_then_marked_failed(db: &Fresh) {
    let store = open(db);
    enqueue(store.as_ref(), &job("bad", "broken"), 0).unwrap();
    let worker = Worker::new("w").lease_ms(1_000).retry_delay_ms(100).handle(
        "broken",
        Box::new(|_, _| Box::pin(async { Err("no luck".into()) })),
    );
    assert!(tick(&worker, store.as_ref(), 0).unwrap());
    assert_eq!(
        Some(JobState::Queued),
        state(store.as_ref(), "bad").unwrap()
    );
    assert!(
        !tick(&worker, store.as_ref(), 99).unwrap(),
        "first retry waits 100 ms"
    );
    assert!(tick(&worker, store.as_ref(), 100).unwrap());
    assert!(
        !tick(&worker, store.as_ref(), 299).unwrap(),
        "second retry waits 200 ms"
    );
    assert!(tick(&worker, store.as_ref(), 300).unwrap());
    assert_eq!(
        Some(JobState::Failed),
        state(store.as_ref(), "bad").unwrap()
    );
    assert!(
        !tick(&worker, store.as_ref(), 1_000_000).unwrap(),
        "a failed job is not tried again"
    );
    let row = &on(&*store)
        .query(
            "SELECT attempts, last_error FROM ortus_jobs WHERE id = 'bad'",
            &[],
        )
        .unwrap()[0];
    assert_eq!((Some(3), Some("no luck")), (row.int(0), row.text(1)));

    // A job whose worker dies on its last try is marked failed too.
    enqueue(
        store.as_ref(),
        &NewJob {
            max_attempts: 1,
            ..job("dies", "count")
        },
        2_000_000,
    )
    .unwrap();
    let taken = claim(store.as_ref(), "gone", &[], 2_000_000, 1_000)
        .unwrap()
        .unwrap();
    assert_eq!((1, 1), (taken.attempt, taken.max_attempts));
    assert_eq!(
        None,
        claim(store.as_ref(), "w", &[], 2_002_000, 1_000).unwrap()
    );
    assert_eq!(
        Some(JobState::Failed),
        state(store.as_ref(), "dies").unwrap()
    );
    assert!(
        !fail(store.as_ref(), &taken, "gone", "late", 2_003_000, 2_004_000).unwrap(),
        "the dead worker cannot change it any more"
    );
}

fn a_failed_handler_leaves_no_writes_behind(db: &Fresh) {
    let store = open(db);
    enqueue(store.as_ref(), &job("half", "half"), 0).unwrap();
    let worker = Worker::new("w").retry_delay_ms(100).handle(
        "half",
        Box::new(|job, tx| {
            Box::pin(async move {
                tx.execute(
                    "INSERT INTO runs (job, times) VALUES ($1, 1)",
                    &[job.id.as_str().into()],
                )
                .await
                .map_err(|e| e.to_string())?;
                if job.attempt == 1 {
                    return Err("crashed halfway".into());
                }
                Ok(())
            })
        }),
    );
    assert!(tick(&worker, store.as_ref(), 0).unwrap());
    assert!(
        runs(store.as_ref()).is_empty(),
        "the first try's write must be rolled back"
    );
    assert!(tick(&worker, store.as_ref(), 100).unwrap());
    assert_eq!(vec![("half".to_string(), 1)], runs(store.as_ref()));
    assert_eq!(Some(JobState::Done), state(store.as_ref(), "half").unwrap());
}

fn many_workers_run_every_job_once(db: &Fresh) {
    let store = open(db);
    for n in 0..60 {
        enqueue(store.as_ref(), &job(&format!("job-{n:02}"), "count"), 0).unwrap();
    }
    let handles: Vec<_> = (0..6)
        .map(|n| {
            // Each worker is its own program with its own connection.
            let store = (db.open)();
            std::thread::spawn(move || {
                let worker = counting_worker(&format!("worker-{n}"));
                let mut done = 0;
                while tick(&worker, store.as_ref(), 10).unwrap() {
                    done += 1;
                }
                done
            })
        })
        .collect();
    let total: i32 = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .sum();
    assert_eq!(60, total);
    let counted = runs(store.as_ref());
    assert_eq!(60, counted.len());
    assert!(counted.iter().all(|(_, times)| *times == 1), "{counted:?}");
    assert_eq!(
        60,
        on(&*store)
            .query("SELECT id FROM ortus_jobs WHERE state = 'done'", &[])
            .unwrap()
            .len()
    );
}

fn a_job_enqueued_in_a_rolled_back_transaction_does_not_exist(db: &Fresh) {
    let store = open(db);
    let result = on(&*store).transaction(&mut |tx| {
        assert!(enqueue_in(tx, &job("ghost", "count"), 0)?);
        tx.execute(
            "INSERT INTO runs (job, times) VALUES ($1, $2)",
            &[Value::from("ghost"), 0.into()],
        )?;
        Err(StoreError::Database("caller changed its mind".into()))
    });
    assert!(result.is_err());
    assert_eq!(None, state(store.as_ref(), "ghost").unwrap());
    assert!(!tick(&counting_worker("w"), store.as_ref(), 10).unwrap());
}

fn jobs_of_other_kinds_never_hide_a_workers_jobs(db: &Fresh) {
    let store = open(db);
    // Many older due jobs that this worker has no handler for.
    for i in 0..50 {
        let id = format!("other-{i:02}");
        assert!(enqueue(store.as_ref(), &job(&id, "someone-else"), 0).unwrap());
    }
    let mut mine = job("mine", "count");
    mine.run_at = 1;
    assert!(enqueue(store.as_ref(), &mine, 0).unwrap());
    assert!(tick(&counting_worker("w"), store.as_ref(), 10).unwrap());
    assert_eq!(Some(JobState::Done), state(store.as_ref(), "mine").unwrap());
    assert_eq!(
        Some(JobState::Queued),
        state(store.as_ref(), "other-00").unwrap()
    );
}

/// A job that outlasts its lease (an outside job: a slow mail server)
/// renews it while it works: another worker does not take it meanwhile.
fn a_slow_job_keeps_its_lease(db: &Fresh) {
    let store = open(db);
    assert!(enqueue(store.as_ref(), &job("slow", "slow"), 0).unwrap());
    let slow = Worker::new("first").lease_ms(300).handle_outside(
        "slow",
        Box::new(|_job| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
                Ok(())
            })
        }),
    );
    let other = Worker::new("second")
        .lease_ms(300)
        .handle_outside("slow", Box::new(|_job| Box::pin(async { Ok(()) })));
    let started = std::time::Instant::now();
    let s2 = Arc::clone(&store);
    let first = std::thread::spawn(move || {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        ortus_core::runtime::block_on(slow.tick(s2.as_ref(), now)).unwrap()
    });
    // Well past the first lease: the job is still held.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        !ortus_core::runtime::block_on(other.tick(store.as_ref(), now)).unwrap(),
        "taken twice"
    );
    assert!(first.join().unwrap());
    assert!(started.elapsed().as_millis() >= 1_200);
    assert_eq!(Some(JobState::Done), state(store.as_ref(), "slow").unwrap());
}

/// Done jobs go after a week, failed ones after a month; queued ones stay.
fn finished_jobs_are_forgotten(db: &Fresh) {
    let store = open(db);
    let day = 24 * 60 * 60 * 1000i64;
    // A real clock (times before 2020 are a test clock's and stay).
    let now = 1_800_000_000_000i64;
    for (id, st, finished) in [
        ("old-done", "done", now - 8 * day),
        ("new-done", "done", now - day),
        ("old-failed", "failed", now - 31 * day),
        ("new-failed", "failed", now - 8 * day),
        ("waiting", "queued", 0),
    ] {
        assert!(enqueue(store.as_ref(), &job(id, "count"), 0).unwrap());
        on(&*store)
            .execute(
                "UPDATE ortus_jobs SET state = $1, finished_at = $2 WHERE id = $3",
                &[st.into(), finished.into(), id.into()],
            )
            .unwrap();
    }
    let gone = ortus_core::runtime::block_on(async {
        let mut tx = store.begin().await.unwrap();
        let n = ortus_core::jobs::prune_in(tx.as_mut(), now).await.unwrap();
        tx.commit().await.unwrap();
        n
    });
    assert_eq!(2, gone);
    for (id, kept) in [
        ("old-done", false),
        ("new-done", true),
        ("old-failed", false),
        ("new-failed", true),
        ("waiting", true),
    ] {
        assert_eq!(kept, state(store.as_ref(), id).unwrap().is_some(), "{id}");
    }
}
