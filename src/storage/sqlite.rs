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
        Self::start(connection)
    }

    /// Opens a database that lives in memory, for tests.
    pub fn in_memory() -> StoreResult<Self> {
        Self::start(Connection::open_in_memory().map_err(error)?)
    }

    fn start(connection: Connection) -> StoreResult<Self> {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        let (commands, inbox) = channel::<Command>();
        std::thread::Builder::new()
            .name("ortus-sqlite".into())
            .spawn(move || {
                // Ends when the store and every open transaction are gone.
                while let Ok(command) = inbox.recv() {
                    run(&connection, command);
                }
            })
            .map_err(|e| StoreError::Database(format!("could not start the SQLite thread: {e}")))?;
        Ok(Self {
            commands,
            turn: Arc::new(Mutex::new(())),
        })
    }
}

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
        Box::pin(self.start_transaction(Command::BeginRead))
    }
}

impl SqliteStore {
    async fn start_transaction(
        &self,
        begin: fn(Reply<()>) -> Command,
    ) -> StoreResult<Box<dyn AsyncTransaction>> {
        let turn = Arc::clone(&self.turn).lock_owned().await;
        let mut transaction = SqliteTransaction {
            commands: self.commands.clone(),
            open: false,
            _turn: turn,
        };
        transaction.ask(begin).await?;
        transaction.open = true;
        Ok(Box::new(transaction) as Box<dyn AsyncTransaction>)
    }
}

struct SqliteTransaction {
    commands: Sender<Command>,
    open: bool,
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
            self.ask(|reply| Command::Execute(sql, params.to_vec(), reply))
                .await
        })
    }

    fn query<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Sqlite);
            self.ask(|reply| Command::Query(sql, params.to_vec(), reply))
                .await
        })
    }
}

impl AsyncTransaction for SqliteTransaction {
    fn commit(mut self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(async move {
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
