//! The PostgreSQL engine, used by the Kubernetes chart.

use std::error::Error;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{to_sql_checked, IsNull, ToSql, Type};
use tokio_postgres::{Client, NoTls};

use super::{
    dialect, AsyncStore, AsyncTransaction, AsyncTx, BoxFuture, Engine, Row, StoreError,
    StoreResult, Value,
};
use crate::runtime;

/// A PostgreSQL database, reached through a small pool of connections.
pub struct PostgresStore {
    pool: Arc<Pool>,
}

struct Pool {
    url: String,
    idle: Mutex<Vec<Client>>,
    /// One permit per connection that may be in use; a transaction waits
    /// here when the pool is fully in use.
    permits: Arc<Semaphore>,
}

impl PostgresStore {
    /// Connects to the database at `url` (`postgres://user:pass@host/db`)
    /// with at most `max_connections` connections.
    pub async fn connect_async(url: &str, max_connections: usize) -> StoreResult<Self> {
        let first = open(url).await?;
        Ok(Self {
            pool: Arc::new(Pool {
                url: url.to_string(),
                idle: Mutex::new(vec![first]),
                permits: Arc::new(Semaphore::new(max_connections.max(1))),
            }),
        })
    }

    /// Blocking form of [`connect_async`](Self::connect_async).
    pub fn connect(url: &str, max_connections: usize) -> StoreResult<Self> {
        runtime::block_on(Self::connect_async(url, max_connections))
    }
}

async fn open(url: &str) -> StoreResult<Client> {
    let (client, connection) = tokio_postgres::connect(url, NoTls).await.map_err(error)?;
    // The connection does its work on the process-wide runtime, so it
    // stays usable whichever runtime the caller is on.
    runtime::io().spawn(connection);
    Ok(client)
}

impl AsyncStore for PostgresStore {
    fn engine(&self) -> Engine {
        Engine::Postgres
    }

    fn begin(&self) -> BoxFuture<'_, StoreResult<Box<dyn AsyncTransaction>>> {
        Box::pin(self.start_transaction("BEGIN"))
    }

    fn begin_read(&self) -> BoxFuture<'_, StoreResult<Box<dyn AsyncTransaction>>> {
        Box::pin(self.start_transaction("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY"))
    }
}

impl PostgresStore {
    async fn start_transaction(&self, begin: &str) -> StoreResult<Box<dyn AsyncTransaction>> {
        let permit = Arc::clone(&self.pool.permits)
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Database("the connection pool is closed".into()))?;
        let idle = self
            .pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop();
        let client = match idle {
            Some(client) if !client.is_closed() => client,
            _ => open(&self.pool.url).await?,
        };
        client.batch_execute(begin).await.map_err(error)?;
        Ok(Box::new(PostgresTransaction {
            client: Some(client),
            pool: Arc::clone(&self.pool),
            _permit: permit,
        }) as Box<dyn AsyncTransaction>)
    }
}

struct PostgresTransaction {
    /// `None` once the transaction has ended. A transaction dropped while
    /// this is still `Some` closes its connection, and the server rolls
    /// the work back.
    client: Option<Client>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
}

impl PostgresTransaction {
    fn client(&self) -> StoreResult<&Client> {
        self.client
            .as_ref()
            .ok_or_else(|| StoreError::Database("the transaction has ended".into()))
    }

    async fn end(mut self: Box<Self>, statement: &str) -> StoreResult<()> {
        let Some(client) = self.client.take() else {
            return Ok(());
        };
        client.batch_execute(statement).await.map_err(error)?;
        self.pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(client);
        Ok(())
    }
}

impl AsyncTx for PostgresTransaction {
    fn execute<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<u64>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Postgres);
            self.client()?
                .execute(sql.as_str(), &refs(params))
                .await
                .map_err(error)
        })
    }

    fn query<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Postgres);
            let rows = self
                .client()?
                .query(sql.as_str(), &refs(params))
                .await
                .map_err(error)?;
            rows.iter().map(read_row).collect()
        })
    }
}

impl AsyncTransaction for PostgresTransaction {
    fn commit(self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(self.end("COMMIT"))
    }

    fn rollback(self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(self.end("ROLLBACK"))
    }
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect()
}

fn read_row(row: &tokio_postgres::Row) -> StoreResult<Row> {
    let mut values = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        let ty = column.type_();
        let value = if *ty == Type::VOID {
            Ok(Value::Null)
        } else if *ty == Type::INT8 {
            row.try_get::<_, Option<i64>>(index).map(Value::from)
        } else if *ty == Type::INT4 {
            row.try_get::<_, Option<i32>>(index)
                .map(|v| Value::from(v.map(i64::from)))
        } else if *ty == Type::INT2 {
            row.try_get::<_, Option<i16>>(index)
                .map(|v| Value::from(v.map(i64::from)))
        } else if *ty == Type::BOOL {
            row.try_get::<_, Option<bool>>(index)
                .map(|v| Value::from(v.map(i64::from)))
        } else if *ty == Type::BYTEA {
            row.try_get::<_, Option<Vec<u8>>>(index).map(Value::from)
        } else {
            row.try_get::<_, Option<String>>(index).map(Value::from)
        };
        values.push(value.map_err(error)?);
    }
    Ok(Row(values))
}

impl ToSql for Value {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut tokio_postgres::types::private::BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        match self {
            Value::Null => Ok(IsNull::Yes),
            Value::Int(v) if *ty == Type::INT4 => i32::try_from(*v)?.to_sql(ty, out),
            Value::Int(v) if *ty == Type::INT2 => i16::try_from(*v)?.to_sql(ty, out),
            Value::Int(v) if *ty == Type::BOOL => (*v != 0).to_sql(ty, out),
            Value::Int(v) => v.to_sql(ty, out),
            Value::Text(v) => v.to_sql(ty, out),
            Value::Blob(v) => v.to_sql(ty, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

fn error(e: tokio_postgres::Error) -> StoreError {
    match e.code() {
        Some(code) if *code == SqlState::UNIQUE_VIOLATION => StoreError::Conflict(e.to_string()),
        _ => StoreError::Database(e.to_string()),
    }
}
