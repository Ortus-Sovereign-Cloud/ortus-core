//! The PostgreSQL engine, used by the Kubernetes chart.

use std::error::Error;
use std::sync::{Condvar, Mutex};

use postgres::error::SqlState;
use postgres::types::{to_sql_checked, IsNull, ToSql, Type};
use postgres::{Client, NoTls};

use super::{dialect, Engine, Row, Store, StoreError, StoreResult, Tx, Value};

/// A PostgreSQL database, reached through a small pool of connections.
pub struct PostgresStore {
    url: String,
    idle: Mutex<Vec<Client>>,
    /// Connections that may still be opened; a transaction waits here when
    /// the pool is fully in use.
    free_slots: Mutex<usize>,
    slot_freed: Condvar,
}

impl PostgresStore {
    /// Connects to the database at `url` (`postgres://user:pass@host/db`)
    /// with at most `max_connections` connections.
    pub fn connect(url: &str, max_connections: usize) -> StoreResult<Self> {
        let first = Client::connect(url, NoTls).map_err(error)?;
        let max = max_connections.max(1);
        Ok(Self {
            url: url.to_string(),
            idle: Mutex::new(vec![first]),
            free_slots: Mutex::new(max - 1),
            slot_freed: Condvar::new(),
        })
    }

    fn checkout(&self) -> StoreResult<Client> {
        loop {
            if let Some(client) = self.idle.lock().unwrap_or_else(|p| p.into_inner()).pop() {
                if !client.is_closed() {
                    return Ok(client);
                }
                self.release_slot();
                continue;
            }
            let mut free = self.free_slots.lock().unwrap_or_else(|p| p.into_inner());
            if *free > 0 {
                *free -= 1;
                drop(free);
                return Client::connect(&self.url, NoTls).map_err(|e| {
                    self.release_slot();
                    error(e)
                });
            }
            // Every connection is in use: wait for one to come back.
            let _unused = self
                .slot_freed
                .wait_timeout(free, std::time::Duration::from_millis(50))
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    fn checkin(&self, client: Client) {
        self.idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(client);
        self.slot_freed.notify_one();
    }

    fn release_slot(&self) {
        *self.free_slots.lock().unwrap_or_else(|p| p.into_inner()) += 1;
        self.slot_freed.notify_one();
    }
}

impl Store for PostgresStore {
    fn engine(&self) -> Engine {
        Engine::Postgres
    }

    fn transaction(&self, work: &mut dyn FnMut(&mut dyn Tx) -> StoreResult<()>) -> StoreResult<()> {
        let mut client = self.checkout()?;
        let result = (|| {
            let mut transaction = client.transaction().map_err(error)?;
            work(&mut PostgresTx {
                transaction: &mut transaction,
            })?;
            transaction.commit().map_err(error)
        })();
        // A dropped transaction rolls back, so the connection is clean
        // either way and can serve the next caller.
        self.checkin(client);
        result
    }
}

struct PostgresTx<'a, 'b> {
    transaction: &'a mut postgres::Transaction<'b>,
}

impl Tx for PostgresTx<'_, '_> {
    fn execute(&mut self, sql: &str, params: &[Value]) -> StoreResult<u64> {
        let sql = dialect(sql, Engine::Postgres);
        self.transaction
            .execute(sql.as_str(), &refs(params))
            .map_err(error)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> StoreResult<Vec<Row>> {
        let sql = dialect(sql, Engine::Postgres);
        let rows = self
            .transaction
            .query(sql.as_str(), &refs(params))
            .map_err(error)?;
        rows.iter().map(read_row).collect()
    }
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect()
}

fn read_row(row: &postgres::Row) -> StoreResult<Row> {
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
        out: &mut postgres::types::private::BytesMut,
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

fn error(e: postgres::Error) -> StoreError {
    match e.code() {
        Some(code) if *code == SqlState::UNIQUE_VIOLATION => StoreError::Conflict(e.to_string()),
        _ => StoreError::Database(e.to_string()),
    }
}
