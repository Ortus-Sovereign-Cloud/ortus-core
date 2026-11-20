//! The migration runner: brings a database to the schema the code expects.

use super::{AsyncStore, AsyncTx, Engine, StoreError, StoreResult, Value};

/// One step in the history of an app's schema. Versions start at 1 and
/// have no gaps. A migration that has been released is never edited.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    /// Statements in the shared SQL subset, separated by `;`. A `;` inside
    /// a single-quoted literal does not end a statement.
    pub sql: &'static str,
}

/// The whole schema of one part of Ortus: its name in the migration
/// history and every migration it has.
#[derive(Clone, Copy, Debug)]
pub struct Schema {
    pub app: &'static str,
    pub migrations: &'static [Migration],
}

impl Schema {
    /// The newest version this program knows.
    pub fn latest(&self) -> i64 {
        self.migrations.len() as i64
    }
}

/// Brings every schema in `schemas` up to date, in order.
pub async fn install(store: &dyn AsyncStore, schemas: &[Schema], now_ms: i64) -> StoreResult<()> {
    for schema in schemas {
        migrate(store, schema.app, schema.migrations, now_ms).await?;
    }
    Ok(())
}

const HISTORY_TABLE: &str = "CREATE TABLE IF NOT EXISTS ortus_migrations (
    app TEXT NOT NULL,
    version BIGINT NOT NULL,
    name TEXT NOT NULL,
    applied_at BIGINT NOT NULL,
    PRIMARY KEY (app, version)
)";

/// Applies every migration of `app` that the database has not seen yet, in
/// order, and returns how many were applied. Running it again does nothing.
///
/// All of an app's pending migrations run in one transaction: after a
/// failure the database is exactly as it was. Two programs that start at
/// the same time do not migrate at once; the second waits and then finds
/// nothing to do.
pub async fn migrate(
    store: &dyn AsyncStore,
    app: &str,
    migrations: &[Migration],
    now_ms: i64,
) -> StoreResult<usize> {
    for (index, migration) in migrations.iter().enumerate() {
        if migration.version != index as i64 + 1 {
            return Err(StoreError::Migration(format!("{app}: migration {} ({}) is at position {}; versions must be 1, 2, 3, ... with no gaps", migration.version, migration.name, index + 1)));
        }
    }
    let mut tx = store.begin().await?;
    // An error returns early and drops the transaction, which rolls back.
    if store.engine() == Engine::Postgres {
        // One migrator at a time across every program on this database.
        tx.execute("SELECT pg_advisory_xact_lock(7310158203)", &[])
            .await?;
    }
    tx.execute(HISTORY_TABLE, &[]).await?;
    let done = history(&mut *tx, app).await?;
    for (version, name) in &done {
        match migrations.get(*version as usize - 1) {
            Some(known) if known.name == name => {}
            Some(known) => return Err(StoreError::Migration(format!("{app}: migration {version} was applied as '{name}' but the code calls it '{}'", known.name))),
            None => return Err(StoreError::Migration(format!("{app}: the database is at migration {version} ('{name}'), newer than this program knows; upgrade the program"))),
        }
    }
    let mut applied = 0;
    for migration in &migrations[done.len()..] {
        for statement in statements(migration.sql) {
            tx.execute(statement, &[]).await?;
        }
        tx.execute(
            "INSERT INTO ortus_migrations (app, version, name, applied_at) VALUES ($1, $2, $3, $4)",
            &[
                app.into(),
                migration.version.into(),
                migration.name.into(),
                now_ms.into(),
            ],
        )
        .await?;
        applied += 1;
    }
    tx.commit().await?;
    Ok(applied)
}

/// The versions of `app` already applied, oldest first.
pub async fn applied_versions(store: &dyn AsyncStore, app: &str) -> StoreResult<Vec<i64>> {
    let mut tx = store.begin().await?;
    tx.execute(HISTORY_TABLE, &[]).await?;
    let versions = history(&mut *tx, app)
        .await?
        .into_iter()
        .map(|(version, _)| version)
        .collect();
    tx.commit().await?;
    Ok(versions)
}

async fn history(tx: &mut dyn AsyncTx, app: &str) -> StoreResult<Vec<(i64, String)>> {
    let rows = tx
        .query(
            "SELECT version, name FROM ortus_migrations WHERE app = $1 ORDER BY version",
            &[Value::from(app)],
        )
        .await?;
    let mut done = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let version = row.int(0).unwrap_or_default();
        if version != index as i64 + 1 {
            return Err(StoreError::Migration(format!(
                "{app}: the migration history in the database has a gap before version {version}"
            )));
        }
        done.push((version, row.text(1).unwrap_or_default().to_string()));
    }
    Ok(done)
}

/// Splits migration SQL into statements at each `;` outside a literal.
fn statements(sql: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_literal = false;
    for (index, c) in sql.char_indices() {
        match c {
            '\'' => in_literal = !in_literal,
            ';' if !in_literal => {
                out.push(&sql[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    out.push(&sql[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::statements;

    #[test]
    fn statements_split_outside_literals_only() {
        assert_eq!(
            vec!["CREATE TABLE a (x TEXT)", "INSERT INTO a VALUES ('b; c')"],
            statements("CREATE TABLE a (x TEXT);\n INSERT INTO a VALUES ('b; c') ; ")
        );
        assert!(statements("  ;; ").is_empty());
    }
}
