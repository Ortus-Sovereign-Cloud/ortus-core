//! Background jobs: work that runs later, survives a crash and is not lost.
//!
//! Jobs live in a table in the database, so there is no separate queue to
//! install. Any program with the worker role takes jobs from the table.
//!
//! What "runs exactly once" means here: a job's handler writes through the
//! transaction it is given, and those writes commit together with the
//! job's completion. If the program dies first, nothing was written and
//! the job runs again. Effects outside the database (sending a mail,
//! calling another server) can therefore happen more than once after a
//! crash; handlers that have such effects must make them safe to repeat.
//!
//! The interface is async. Code that is not async uses [`blocking`].

use std::collections::HashMap;

use crate::storage::{
    migrate, AsyncStore, AsyncTransaction, AsyncTx, BoxFuture, Migration, StoreResult, Value,
};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "jobs",
    sql: "CREATE TABLE ortus_jobs (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        state TEXT NOT NULL,
        run_at BIGINT NOT NULL,
        attempts BIGINT NOT NULL,
        max_attempts BIGINT NOT NULL,
        locked_by TEXT,
        locked_until BIGINT,
        last_error TEXT,
        created_at BIGINT NOT NULL,
        finished_at BIGINT
    );
    CREATE INDEX ortus_jobs_due ON ortus_jobs (state, run_at)",
}];

/// Creates or upgrades the job table.
pub async fn install(store: &dyn AsyncStore, now_ms: i64) -> StoreResult<usize> {
    migrate(store, "core.jobs", MIGRATIONS, now_ms).await
}

/// A job to add to the queue.
#[derive(Clone, Debug)]
pub struct NewJob<'a> {
    /// Unique id chosen by the caller. Adding the same id twice adds one
    /// job, so a caller that retries does not duplicate work.
    pub id: &'a str,
    /// Which handler runs the job.
    pub kind: &'a str,
    /// Data for the handler, as text (usually JSON).
    pub payload: &'a str,
    /// Earliest time to run, in Unix milliseconds.
    pub run_at: i64,
    /// How many times the job may be tried before it is marked failed.
    pub max_attempts: i64,
}

/// A job taken from the queue by a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub payload: String,
    /// 1 on the first try.
    pub attempt: i64,
    pub max_attempts: i64,
}

/// Where a job is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
}

/// Adds a job. Returns `false` when a job with this id already exists.
pub async fn enqueue(store: &dyn AsyncStore, job: &NewJob<'_>, now_ms: i64) -> StoreResult<bool> {
    let mut tx = store.begin().await?;
    let added = enqueue_in(&mut *tx, job, now_ms).await?;
    tx.commit().await?;
    Ok(added)
}

/// Adds a job inside the caller's transaction, so the job exists exactly
/// when the caller's other writes do.
pub async fn enqueue_in(tx: &mut dyn AsyncTx, job: &NewJob<'_>, now_ms: i64) -> StoreResult<bool> {
    if !tx
        .query("SELECT id FROM ortus_jobs WHERE id = $1", &[job.id.into()])
        .await?
        .is_empty()
    {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO ortus_jobs (id, kind, payload, state, run_at, attempts, max_attempts, created_at) VALUES ($1, $2, $3, 'queued', $4, 0, $5, $6)",
        &[job.id.into(), job.kind.into(), job.payload.into(), job.run_at.into(), job.max_attempts.max(1).into(), now_ms.into()],
    )
    .await?;
    Ok(true)
}

/// The state of a job, or `None` when there is no job with this id.
pub async fn state(store: &dyn AsyncStore, id: &str) -> StoreResult<Option<JobState>> {
    let rows = store
        .query("SELECT state FROM ortus_jobs WHERE id = $1", &[id.into()])
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.text(0))
        .map(|state| match state {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "done" => JobState::Done,
            _ => JobState::Failed,
        }))
}

/// A job that is due and free, or whose worker stopped answering.
const DUE: &str =
    "((state = 'queued' AND run_at <= $1) OR (state = 'running' AND locked_until <= $1))";

/// Takes the next due job for `worker` and holds it for `lease_ms`. A job
/// whose worker died is due again when its lease runs out. With `kinds`
/// empty, any kind of job is taken.
pub async fn claim(
    store: &dyn AsyncStore,
    worker: &str,
    kinds: &[&str],
    now_ms: i64,
    lease_ms: i64,
) -> StoreResult<Option<Job>> {
    // Several workers may pick the same candidate; the UPDATE below checks
    // the job is still due, so only one of them gets it and the others
    // look again.
    for _ in 0..8 {
        let mut tx = store.begin().await?;
        let candidates = tx.query(&format!("SELECT id, kind, payload, attempts, max_attempts FROM ortus_jobs WHERE {DUE} ORDER BY run_at, id LIMIT 32"), &[now_ms.into()]).await?;
        let Some(row) = candidates
            .iter()
            .find(|row| kinds.is_empty() || kinds.contains(&row.text(1).unwrap_or_default()))
        else {
            tx.commit().await?;
            return Ok(None);
        };
        let id = row.text(0).unwrap_or_default().to_string();
        let attempts = row.int(3).unwrap_or_default();
        let max_attempts = row.int(4).unwrap_or(1);
        if attempts >= max_attempts {
            // It was running its last try when its worker died.
            tx.execute(&format!("UPDATE ortus_jobs SET state = 'failed', finished_at = $1, last_error = 'worker stopped during the last attempt', locked_by = NULL, locked_until = NULL WHERE id = $2 AND {DUE}"), &[now_ms.into(), id.into()]).await?;
            tx.commit().await?;
            continue;
        }
        let taken = tx.execute(&format!("UPDATE ortus_jobs SET state = 'running', attempts = attempts + 1, locked_by = $2, locked_until = $3 WHERE id = $4 AND {DUE}"), &[now_ms.into(), worker.into(), (now_ms + lease_ms).into(), id.clone().into()]).await?;
        tx.commit().await?;
        if taken == 1 {
            return Ok(Some(Job {
                id,
                kind: row.text(1).unwrap_or_default().to_string(),
                payload: row.text(2).unwrap_or_default().to_string(),
                attempt: attempts + 1,
                max_attempts,
            }));
        }
    }
    Ok(None)
}

/// Starts finishing a job: returns a transaction in which the job is
/// already marked done. The caller writes the job's results through it and
/// commits; results and completion land together or not at all.
///
/// Returns `None` when this worker no longer holds the job (its lease ran
/// out and another worker took it).
pub async fn start_completion(
    store: &dyn AsyncStore,
    job: &Job,
    worker: &str,
    now_ms: i64,
) -> StoreResult<Option<Box<dyn AsyncTransaction>>> {
    let mut tx = store.begin().await?;
    let held = tx.execute("UPDATE ortus_jobs SET state = 'done', finished_at = $1, locked_by = NULL, locked_until = NULL, last_error = NULL WHERE id = $2 AND state = 'running' AND locked_by = $3 AND attempts = $4", &[now_ms.into(), job.id.as_str().into(), worker.into(), job.attempt.into()]).await?;
    if held != 1 {
        tx.rollback().await?;
        return Ok(None);
    }
    Ok(Some(tx))
}

/// Records that a try failed. The job is queued again for `retry_at`, or
/// marked failed when it has no tries left. Returns `false` when this
/// worker no longer holds the job.
pub async fn fail(
    store: &dyn AsyncStore,
    job: &Job,
    worker: &str,
    error: &str,
    now_ms: i64,
    retry_at: i64,
) -> StoreResult<bool> {
    let held: Vec<Value> = vec![job.id.as_str().into(), worker.into(), job.attempt.into()];
    let changed = if job.attempt >= job.max_attempts {
        store.execute("UPDATE ortus_jobs SET state = 'failed', finished_at = $4, last_error = $5, locked_by = NULL, locked_until = NULL WHERE id = $1 AND state = 'running' AND locked_by = $2 AND attempts = $3", &[held, vec![now_ms.into(), error.into()]].concat()).await?
    } else {
        store.execute("UPDATE ortus_jobs SET state = 'queued', run_at = $4, last_error = $5, locked_by = NULL, locked_until = NULL WHERE id = $1 AND state = 'running' AND locked_by = $2 AND attempts = $3", &[held, vec![retry_at.into(), error.into()]].concat()).await?
    };
    Ok(changed == 1)
}

/// What a handler does with a job. Its database writes go through the
/// transaction and commit together with the job's completion.
pub type Handler = Box<
    dyn for<'a> Fn(&'a Job, &'a mut dyn AsyncTx) -> BoxFuture<'a, Result<(), String>> + Send + Sync,
>;

/// Takes jobs from the queue and runs their handlers.
pub struct Worker {
    name: String,
    handlers: HashMap<String, Handler>,
    lease_ms: i64,
    retry_delay_ms: i64,
}

impl Worker {
    /// A worker with a name that is unique among running workers.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            handlers: HashMap::new(),
            lease_ms: 60_000,
            retry_delay_ms: 5_000,
        }
    }

    /// How long a job is held before another worker may take it over. Set
    /// it longer than the slowest handler.
    pub fn lease_ms(mut self, lease_ms: i64) -> Self {
        self.lease_ms = lease_ms;
        self
    }

    /// Base delay before a failed job is tried again; it doubles per try.
    pub fn retry_delay_ms(mut self, retry_delay_ms: i64) -> Self {
        self.retry_delay_ms = retry_delay_ms;
        self
    }

    /// Registers the handler for one kind of job.
    pub fn handle(mut self, kind: &str, handler: Handler) -> Self {
        self.handlers.insert(kind.to_string(), handler);
        self
    }

    /// Runs at most one due job. Returns `false` when nothing was due.
    pub async fn tick(&self, store: &dyn AsyncStore, now_ms: i64) -> StoreResult<bool> {
        let kinds: Vec<&str> = self.handlers.keys().map(String::as_str).collect();
        let Some(job) = claim(store, &self.name, &kinds, now_ms, self.lease_ms).await? else {
            return Ok(false);
        };
        let Some(mut tx) = start_completion(store, &job, &self.name, now_ms).await? else {
            return Ok(true);
        };
        let handler = &self.handlers[&job.kind];
        let failure = match handler(&job, &mut *tx).await {
            Ok(()) => tx.commit().await.err().map(|e| e.to_string()),
            Err(message) => {
                tx.rollback().await?;
                Some(message)
            }
        };
        if let Some(message) = failure {
            let delay = self
                .retry_delay_ms
                .saturating_mul(1 << (job.attempt - 1).clamp(0, 16));
            fail(
                store,
                &job,
                &self.name,
                &message,
                now_ms,
                now_ms.saturating_add(delay),
            )
            .await?;
        }
        Ok(true)
    }

    /// Runs due jobs until `stop` returns `true`, sleeping `idle` between
    /// looks when the queue is empty.
    pub async fn run(
        &self,
        store: &dyn AsyncStore,
        idle: std::time::Duration,
        stop: &(dyn Fn() -> bool + Sync),
    ) {
        while !stop() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            match self.tick(store, now_ms).await {
                Ok(true) => {}
                Ok(false) | Err(_) => tokio::time::sleep(idle).await,
            }
        }
    }
}

/// The job interface for code that is not async. Do not call these from
/// async code.
pub mod blocking {
    use super::{Job, JobState, NewJob, Worker};
    use crate::runtime::block_on;
    use crate::storage::blocking::Tx;
    use crate::storage::{AsyncStore, StoreResult};

    pub fn install(store: &dyn AsyncStore, now_ms: i64) -> StoreResult<usize> {
        block_on(super::install(store, now_ms))
    }

    pub fn enqueue(store: &dyn AsyncStore, job: &NewJob<'_>, now_ms: i64) -> StoreResult<bool> {
        block_on(super::enqueue(store, job, now_ms))
    }

    pub fn enqueue_in(tx: &mut dyn Tx, job: &NewJob<'_>, now_ms: i64) -> StoreResult<bool> {
        block_on(super::enqueue_in(tx.async_tx(), job, now_ms))
    }

    pub fn state(store: &dyn AsyncStore, id: &str) -> StoreResult<Option<JobState>> {
        block_on(super::state(store, id))
    }

    pub fn claim(
        store: &dyn AsyncStore,
        worker: &str,
        kinds: &[&str],
        now_ms: i64,
        lease_ms: i64,
    ) -> StoreResult<Option<Job>> {
        block_on(super::claim(store, worker, kinds, now_ms, lease_ms))
    }

    /// Runs `work` and marks the job done in one transaction. Returns
    /// `false`, with nothing written, when this worker no longer holds
    /// the job.
    pub fn complete(
        store: &dyn AsyncStore,
        job: &Job,
        worker: &str,
        now_ms: i64,
        work: &mut dyn FnMut(&mut dyn Tx) -> StoreResult<()>,
    ) -> StoreResult<bool> {
        let Some(mut tx) = block_on(super::start_completion(store, job, worker, now_ms))? else {
            return Ok(false);
        };
        match work(&mut crate::storage::blocking::BlockingTx { tx: &mut *tx }) {
            Ok(()) => block_on(tx.commit()).map(|()| true),
            Err(e) => {
                let _ = block_on(tx.rollback());
                Err(e)
            }
        }
    }

    pub fn fail(
        store: &dyn AsyncStore,
        job: &Job,
        worker: &str,
        error: &str,
        now_ms: i64,
        retry_at: i64,
    ) -> StoreResult<bool> {
        block_on(super::fail(store, job, worker, error, now_ms, retry_at))
    }

    /// Blocking form of [`Worker::tick`].
    pub fn tick(worker: &Worker, store: &dyn AsyncStore, now_ms: i64) -> StoreResult<bool> {
        block_on(worker.tick(store, now_ms))
    }
}
