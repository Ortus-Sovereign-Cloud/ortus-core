//! The storage interface for code that is not async.
//!
//! [`on`] gives the blocking interface to any [`AsyncStore`]: the same
//! database, with calls that wait for the result. Do not call these from
//! async code.

use super::{AsyncStore, AsyncTx, Engine, Migration, Row, StoreResult, Value};
use crate::runtime::block_on;

/// Statements that run inside one transaction.
pub trait Tx {
    /// Runs a statement and returns the number of rows it changed.
    fn execute(&mut self, sql: &str, params: &[Value]) -> StoreResult<u64>;
    /// Runs a query and returns its rows.
    fn query(&mut self, sql: &str, params: &[Value]) -> StoreResult<Vec<Row>>;
    /// The async transaction underneath, for blocking wrappers of async
    /// functions that take one.
    fn async_tx(&mut self) -> &mut dyn AsyncTx;
}

/// A database, for code that is not async: `on(store).execute(...)`.
#[derive(Clone, Copy)]
pub struct Store<'a> {
    store: &'a dyn AsyncStore,
}

/// The blocking interface to `store`.
pub fn on(store: &dyn AsyncStore) -> Store<'_> {
    Store { store }
}

impl Store<'_> {
    pub fn engine(&self) -> Engine {
        self.store.engine()
    }

    /// Runs `work` in a transaction. The transaction commits when `work`
    /// returns `Ok` and rolls back when it returns `Err`.
    pub fn transaction(
        &self,
        work: &mut dyn FnMut(&mut dyn Tx) -> StoreResult<()>,
    ) -> StoreResult<()> {
        let mut tx = block_on(self.store.begin())?;
        match work(&mut BlockingTx { tx: &mut *tx }) {
            Ok(()) => block_on(tx.commit()),
            Err(e) => {
                // The caller's error is the one worth reporting.
                let _ = block_on(tx.rollback());
                Err(e)
            }
        }
    }

    /// Runs one statement in its own transaction.
    pub fn execute(&self, sql: &str, params: &[Value]) -> StoreResult<u64> {
        block_on(self.store.execute(sql, params))
    }

    /// Runs one query in its own transaction.
    pub fn query(&self, sql: &str, params: &[Value]) -> StoreResult<Vec<Row>> {
        block_on(self.store.query(sql, params))
    }
}

pub(crate) struct BlockingTx<'a> {
    pub(crate) tx: &'a mut dyn AsyncTx,
}

impl Tx for BlockingTx<'_> {
    fn execute(&mut self, sql: &str, params: &[Value]) -> StoreResult<u64> {
        block_on(self.tx.execute(sql, params))
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> StoreResult<Vec<Row>> {
        block_on(self.tx.query(sql, params))
    }

    fn async_tx(&mut self) -> &mut dyn AsyncTx {
        self.tx
    }
}

/// Blocking form of [`migrate`](super::migrate).
pub fn migrate(
    store: &dyn AsyncStore,
    app: &str,
    migrations: &[Migration],
    now_ms: i64,
) -> StoreResult<usize> {
    block_on(super::migrate(store, app, migrations, now_ms))
}

/// Blocking form of [`applied_versions`](super::applied_versions).
pub fn applied_versions(store: &dyn AsyncStore, app: &str) -> StoreResult<Vec<i64>> {
    block_on(super::applied_versions(store, app))
}
