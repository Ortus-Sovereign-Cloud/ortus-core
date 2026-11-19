//! The SQLite engine, used by the single-machine program.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, ErrorCode, TransactionBehavior};

use super::{dialect, Engine, Row, Store, StoreError, StoreResult, Tx, Value};

/// A SQLite database in one file. All work goes through one connection:
/// SQLite allows one writer at a time, and write-ahead logging keeps the
/// file consistent across a crash.
pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    /// Opens (and creates, when missing) the database file at `path`.
    pub fn open(path: &Path) -> StoreResult<Self> {
        let connection = Connection::open(path).map_err(error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(error)?;
        Self::configure(connection)
    }

    /// Opens a database that lives in memory, for tests.
    pub fn in_memory() -> StoreResult<Self> {
        Self::configure(Connection::open_in_memory().map_err(error)?)
    }

    fn configure(connection: Connection) -> StoreResult<Self> {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

impl Store for SqliteStore {
    fn engine(&self) -> Engine {
        Engine::Sqlite
    }

    fn transaction(&self, work: &mut dyn FnMut(&mut dyn Tx) -> StoreResult<()>) -> StoreResult<()> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // IMMEDIATE takes the write lock at the start, so two programs on
        // one file queue up instead of failing halfway through.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        work(&mut SqliteTx {
            transaction: &transaction,
        })?;
        transaction.commit().map_err(error)
    }
}

struct SqliteTx<'a> {
    transaction: &'a rusqlite::Transaction<'a>,
}

impl Tx for SqliteTx<'_> {
    fn execute(&mut self, sql: &str, params: &[Value]) -> StoreResult<u64> {
        let sql = dialect(sql, Engine::Sqlite);
        let changed = self
            .transaction
            .execute(&sql, rusqlite::params_from_iter(params.iter().map(to_sql)))
            .map_err(error)?;
        Ok(changed as u64)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> StoreResult<Vec<Row>> {
        let sql = dialect(sql, Engine::Sqlite);
        let mut statement = self.transaction.prepare(&sql).map_err(error)?;
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
