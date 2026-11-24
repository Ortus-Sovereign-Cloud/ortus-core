//! The SQLite engine, used by the single-machine program.

use std::path::Path;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, ErrorCode};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use super::{
    dialect, AsyncStore, AsyncTransaction, AsyncTx, BoxFuture, Engine, Row, StoreError,
    StoreResult, Value,
};

/// A SQLite database in one file.
///
/// SQLite itself is not async, so the connection lives on its own thread
/// and statements are sent to it. One transaction runs at a time: SQLite
/// allows one writer, and write-ahead logging keeps the file consistent
/// across a crash.
pub struct SqliteStore {
    commands: Sender<Command>,
    /// Held by the open transaction; the next one waits for it.
    turn: Arc<Mutex<()>>,
    /// A second connection, for reads only (a file database): write-ahead
    /// logging lets it read one consistent snapshot while the writer works,
    /// so pages and backups do not queue behind writes, nor writes behind
    /// an hour's restore point.
    reader: Option<(Sender<Command>, Arc<Mutex<()>>)>,
}

type Reply<T> = oneshot::Sender<StoreResult<T>>;

enum Command {
    Begin(Reply<()>),
    BeginRead(Reply<()>),
    Execute(String, Vec<Value>, Reply<u64>),
    Query(String, Vec<Value>, Reply<Vec<Row>>),
    Commit(Reply<()>),
    Rollback(Option<Reply<()>>),
}

impl SqliteStore {
    /// Opens (and creates, when missing) the database file at `path`.
    pub fn open(path: &Path) -> StoreResult<Self> {
        let connection = Connection::open(path).map_err(error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(error)?;
        let mut store = Self::start(connection)?;
        let reading = Connection::open(path).map_err(error)?;
        reading
            .pragma_update(None, "query_only", "ON")
            .map_err(error)?;
        reading.busy_timeout(super::BUSY_LIMIT).map_err(error)?;
        store.reader = Some((
            thread(reading, "ortus-sqlite-read")?,
            Arc::new(Mutex::new(())),
        ));
        Ok(store)
    }

    /// Opens a database that lives in memory, for tests (one connection).
    pub fn in_memory() -> StoreResult<Self> {
        Self::start(Connection::open_in_memory().map_err(error)?)
    }

    fn start(connection: Connection) -> StoreResult<Self> {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(error)?;
        connection.busy_timeout(super::BUSY_LIMIT).map_err(error)?;
        Ok(Self {
            commands: thread(connection, "ortus-sqlite")?,
            turn: Arc::new(Mutex::new(())),
            reader: None,
        })
    }
}

/// The thread a connection lives on; statements are sent to it.
fn thread(connection: Connection, name: &str) -> StoreResult<Sender<Command>> {
    let (commands, inbox) = channel::<Command>();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            // Ends when the store and every open transaction are gone.
            while let Ok(command) = inbox.recv() {
                run(&connection, command);
            }
        })
        .map_err(|e| StoreError::Database(format!("could not start the SQLite thread: {e}")))?;
    Ok(commands)
}

impl SqliteStore {}

fn run(connection: &Connection, command: Command) {
    // A reply that nobody waits for any more is not an error.
    match command {
        // IMMEDIATE takes the write lock at the start, so two programs on
        // one file queue up instead of failing halfway through.
        Command::Begin(reply) => {
            drop(reply.send(connection.execute_batch("BEGIN IMMEDIATE").map_err(error)))
        }
        // A deferred read transaction in WAL mode reads one snapshot and
        // lets writers in other programs go on.
        Command::BeginRead(reply) => drop(
            reply.send(
                connection
                    .execute_batch("BEGIN DEFERRED; PRAGMA query_only = ON")
                    .map_err(error),
            ),
        ),
        Command::Execute(sql, params, reply) => {
            drop(reply.send(execute(connection, &sql, &params)))
        }
        Command::Query(sql, params, reply) => drop(reply.send(query(connection, &sql, &params))),
        Command::Commit(reply) => drop(
            reply.send(
                connection
                    .execute_batch("COMMIT; PRAGMA query_only = OFF")
                    .map_err(error),
            ),
        ),
        Command::Rollback(reply) => {
            let result = if connection.is_autocommit() {
                Ok(())
            } else {
                connection.execute_batch("ROLLBACK").map_err(error)
            }
            .and_then(|()| {
                connection
                    .execute_batch("PRAGMA query_only = OFF")
                    .map_err(error)
            });
            if let Some(reply) = reply {
                drop(reply.send(result));
            }
        }
    }
}

fn execute(connection: &Connection, sql: &str, params: &[Value]) -> StoreResult<u64> {
    let changed = connection
        .execute(sql, rusqlite::params_from_iter(params.iter().map(to_sql)))
        .map_err(error)?;
    Ok(changed as u64)
}

fn query(connection: &Connection, sql: &str, params: &[Value]) -> StoreResult<Vec<Row>> {
    let mut statement = connection.prepare(sql).map_err(error)?;
    let columns = statement.column_count();
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter().map(to_sql)))
        .map_err(error)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(error)? {
        let mut values = Vec::with_capacity(columns);
        for index in 0..columns {
            values.push(match row.get_ref(index).map_err(error)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(v) => Value::Int(v),
                ValueRef::Real(v) => Value::Text(v.to_string()),
                ValueRef::Text(v) => Value::Text(String::from_utf8_lossy(v).into_owned()),
                ValueRef::Blob(v) => Value::Blob(v.to_vec()),
            });
        }
        out.push(Row(values));
    }
    Ok(out)
}

impl AsyncStore for SqliteStore {
    fn engine(&self) -> Engine {
        Engine::Sqlite
    }

    fn begin(&self) -> BoxFuture<'_, StoreResult<Box<dyn AsyncTransaction>>> {
        Box::pin(self.start_transaction(Command::Begin))
    }

    fn begin_read(&self) -> BoxFuture<'_, StoreResult<Box<dyn AsyncTransaction>>> {
        Box::pin(async move {
            match &self.reader {
                Some((commands, turn)) => start_on(commands, turn, Command::BeginRead).await,
                None => self.start_transaction(Command::BeginRead).await,
            }
        })
    }

    /// One query: on the reading connection when there is one, so a page
    /// never waits for a writer.
    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            let mut tx = match &self.reader {
                Some((commands, turn)) => start_on(commands, turn, Command::BeginRead).await?,
                None => self.start_transaction(Command::Begin).await?,
            };
            let rows = tx.query(sql, params).await?;
            tx.commit().await?;
            Ok(rows)
        })
    }
}

impl SqliteStore {
    async fn start_transaction(
        &self,
        begin: fn(Reply<()>) -> Command,
    ) -> StoreResult<Box<dyn AsyncTransaction>> {
        start_on(&self.commands, &self.turn, begin).await
    }
}

/// Starts a transaction on one connection: one at a time on it. Waiting
/// this long means a transaction waits on itself (code that starts a
/// second transaction while holding one): fail instead of hanging forever.
async fn start_on(
    commands: &Sender<Command>,
    turn: &Arc<Mutex<()>>,
    begin: fn(Reply<()>) -> Command,
) -> StoreResult<Box<dyn AsyncTransaction>> {
    let turn = tokio::time::timeout(super::BUSY_LIMIT, Arc::clone(turn).lock_owned())
        .await
        .map_err(|_| StoreError::Database(super::BUSY_MESSAGE.into()))?;
    let mut transaction = SqliteTransaction {
        commands: commands.clone(),
        open: false,
        failed: false,
        _turn: turn,
    };
    transaction.ask(begin).await?;
    transaction.open = true;
    Ok(Box::new(transaction) as Box<dyn AsyncTransaction>)
}

struct SqliteTransaction {
    commands: Sender<Command>,
    open: bool,
    /// A statement failed: commit refuses, as PostgreSQL does, so code
    /// behaves the same on both engines.
    failed: bool,
    // Declared last: released after `Drop` has queued the rollback.
    _turn: OwnedMutexGuard<()>,
}

impl SqliteTransaction {
    async fn ask<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> StoreResult<T> {
        let stopped = || StoreError::Database("the SQLite thread has stopped".into());
        let (reply, answer) = oneshot::channel();
        self.commands.send(command(reply)).map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())?
    }
}

impl AsyncTx for SqliteTransaction {
    fn execute<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<u64>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Sqlite);
            let started = std::time::Instant::now();
            let result = self
                .ask(|reply| Command::Execute(sql, params.to_vec(), reply))
                .await;
            crate::request::statement(started.elapsed());
            self.failed |= result.is_err();
            result
        })
    }

    fn query<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Sqlite);
            let started = std::time::Instant::now();
            let result = self
                .ask(|reply| Command::Query(sql, params.to_vec(), reply))
                .await;
            crate::request::statement(started.elapsed());
            self.failed |= result.is_err();
            result
        })
    }
}

impl AsyncTransaction for SqliteTransaction {
    fn commit(mut self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(async move {
            if self.failed {
                self.open = false;
                self.ask(|reply| Command::Rollback(Some(reply))).await?;
                return Err(StoreError::Database(
                    "a statement in the transaction failed, so nothing was committed".into(),
                ));
            }
            let result = self.ask(Command::Commit).await;
            if result.is_ok() {
                self.open = false;
            }
            result
        })
    }

    fn rollback(mut self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(async move {
            self.open = false;
            self.ask(|reply| Command::Rollback(Some(reply))).await
        })
    }
}

impl Drop for SqliteTransaction {
    fn drop(&mut self) {
        if self.open {
            // Queued before the turn is released, so it runs before the
            // next transaction's BEGIN.
            let _ = self.commands.send(Command::Rollback(None));
        }
    }
}

fn to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Int(v) => SqlValue::Integer(*v),
        Value::Text(v) => SqlValue::Text(v.clone()),
        Value::Blob(v) => SqlValue::Blob(v.clone()),
    }
}

fn error(e: rusqlite::Error) -> StoreError {
    match &e {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation =>
        {
            StoreError::Conflict(e.to_string())
        }
        _ => StoreError::Database(e.to_string()),
    }
}
