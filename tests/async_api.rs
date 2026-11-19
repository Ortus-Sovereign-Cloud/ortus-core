//! The async interfaces, used directly from a caller's own runtime, on
//! every database engine. The other suites reach the same code through
//! the blocking wrappers.

mod common;

use std::sync::Arc;

use common::{postgres, sqlite, Fresh};
use ortus_core::jobs::{self, JobState, NewJob, Worker};
use ortus_core::storage::{migrate, AsyncStore, Migration, StoreError, Value};

macro_rules! on_both_engines {
    ($($name:ident),* $(,)?) => {
        mod on_sqlite { $( #[test] fn $name() { if let Some(db) = super::sqlite() { super::$name(&db) } } )* }
        mod on_postgres { $( #[test] fn $name() { if let Some(db) = super::postgres() { super::$name(&db) } } )* }
    };
}

on_both_engines!(
    transactions_commit_roll_back_and_roll_back_when_dropped,
    many_tasks_share_one_store,
    a_worker_runs_jobs_from_async_code,
    a_store_outlives_the_runtime_that_used_it
);

const NOTES: Migration = Migration {
    version: 1,
    name: "notes",
    sql: "CREATE TABLE notes (id BIGINT PRIMARY KEY, title TEXT NOT NULL)",
};

/// A runtime of the caller's own, separate from the one Ortus keeps.
fn callers_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap()
}

async fn ids(store: &dyn AsyncStore) -> Vec<i64> {
    store
        .query("SELECT id FROM notes ORDER BY id", &[])
        .await
        .unwrap()
        .iter()
        .map(|row| row.int(0).unwrap())
        .collect()
}

fn transactions_commit_roll_back_and_roll_back_when_dropped(db: &Fresh) {
    let store = (db.open)();
    callers_runtime().block_on(async {
        assert_eq!(
            1,
            migrate(store.as_ref(), "test", &[NOTES], 1).await.unwrap()
        );

        let mut tx = store.begin().await.unwrap();
        tx.execute(
            "INSERT INTO notes (id, title) VALUES ($1, $2)",
            &[1.into(), "kept".into()],
        )
        .await
        .unwrap();
        assert_eq!(
            1,
            tx.query("SELECT id FROM notes", &[]).await.unwrap().len(),
            "a transaction sees its own writes"
        );
        tx.commit().await.unwrap();

        let mut tx = store.begin().await.unwrap();
        tx.execute(
            "INSERT INTO notes (id, title) VALUES ($1, $2)",
            &[2.into(), "rolled back".into()],
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        let mut tx = store.begin().await.unwrap();
        tx.execute(
            "INSERT INTO notes (id, title) VALUES ($1, $2)",
            &[3.into(), "dropped".into()],
        )
        .await
        .unwrap();
        drop(tx);

        // A statement that fails inside a transaction: the early return
        // drops the transaction, and the store keeps working.
        let failed: Result<(), StoreError> = async {
            let mut tx = store.begin().await?;
            tx.execute(
                "INSERT INTO notes (id, title) VALUES ($1, $2)",
                &[4.into(), "before the error".into()],
            )
            .await?;
            tx.execute(
                "INSERT INTO notes (id, title) VALUES ($1, $2)",
                &[1.into(), "duplicate".into()],
            )
            .await?;
            tx.commit().await
        }
        .await;
        assert!(
            matches!(failed, Err(StoreError::Conflict(_))),
            "got {failed:?}"
        );

        assert_eq!(vec![1], ids(store.as_ref()).await);
    });
}

fn many_tasks_share_one_store(db: &Fresh) {
    let store = (db.open)();
    callers_runtime().block_on(async {
        migrate(store.as_ref(), "test", &[NOTES], 1).await.unwrap();
        let tasks: Vec<_> = (0..40i64)
            .map(|id| {
                let store = Arc::clone(&store);
                tokio::spawn(async move {
                    store
                        .execute(
                            "INSERT INTO notes (id, title) VALUES ($1, $2)",
                            &[id.into(), Value::from("n")],
                        )
                        .await
                        .unwrap()
                })
            })
            .collect();
        for task in tasks {
            assert_eq!(1, task.await.unwrap());
        }
        assert_eq!((0..40).collect::<Vec<i64>>(), ids(store.as_ref()).await);
    });
}

fn a_worker_runs_jobs_from_async_code(db: &Fresh) {
    let store = (db.open)();
    callers_runtime().block_on(async {
        jobs::install(store.as_ref(), 0).await.unwrap();
        migrate(store.as_ref(), "test", &[NOTES], 1).await.unwrap();
        for id in ["a", "b", "c"] {
            assert!(jobs::enqueue(
                store.as_ref(),
                &NewJob {
                    id,
                    kind: "note",
                    payload: id,
                    run_at: 0,
                    max_attempts: 2
                },
                0
            )
            .await
            .unwrap());
        }
        let worker = Arc::new(Worker::new("w").handle(
            "note",
            Box::new(|job, tx| {
                Box::pin(async move {
                    let id = i64::from(job.payload.as_bytes()[0]);
                    tx.execute(
                        "INSERT INTO notes (id, title) VALUES ($1, $2)",
                        &[id.into(), job.payload.as_str().into()],
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                })
            }),
        ));
        // Three ticks at once, as three workers in one program would run.
        let ticks: Vec<_> = (0..3)
            .map(|_| {
                let (worker, store) = (Arc::clone(&worker), Arc::clone(&store));
                tokio::spawn(async move { worker.tick(store.as_ref(), 10).await.unwrap() })
            })
            .collect();
        for tick in ticks {
            assert!(tick.await.unwrap());
        }
        assert!(!worker.tick(store.as_ref(), 20).await.unwrap());
        assert_eq!(vec![97, 98, 99], ids(store.as_ref()).await);
        assert_eq!(
            Some(JobState::Done),
            jobs::state(store.as_ref(), "b").await.unwrap()
        );
    });
}

fn a_store_outlives_the_runtime_that_used_it(db: &Fresh) {
    let store = (db.open)();
    let first = callers_runtime();
    first.block_on(async {
        migrate(store.as_ref(), "test", &[NOTES], 1).await.unwrap();
        store
            .execute(
                "INSERT INTO notes (id, title) VALUES ($1, $2)",
                &[1.into(), "first runtime".into()],
            )
            .await
            .unwrap();
    });
    drop(first);
    // The caller's runtime is gone; the store's connections must not be.
    callers_runtime().block_on(async {
        store
            .execute(
                "INSERT INTO notes (id, title) VALUES ($1, $2)",
                &[2.into(), "second runtime".into()],
            )
            .await
            .unwrap();
        assert_eq!(vec![1, 2], ids(store.as_ref()).await);
    });
    // And the blocking interface still works on the same store.
    assert_eq!(
        2,
        ortus_core::storage::blocking::on(store.as_ref())
            .query("SELECT id FROM notes", &[])
            .unwrap()
            .len()
    );
}

#[test]
#[should_panic(expected = "blocking Ortus call was made from async code")]
fn a_blocking_call_from_async_code_is_refused_loudly() {
    let store = sqlite().map(|db| (db.open)()).unwrap();
    callers_runtime().block_on(async {
        let _ = ortus_core::storage::blocking::on(store.as_ref()).query("SELECT 1", &[]);
    });
}
