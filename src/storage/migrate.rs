//! The migration runner: brings a database to the schema the code expects.

use super::{Engine, Store, StoreError, StoreResult, Tx, Value};

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
pub fn migrate(
    store: &dyn Store,
    app: &str,
    migrations: &[Migration],
    now_ms: i64,
) -> StoreResult<usize> {
    for (index, migration) in migrations.iter().enumerate() {
        if migration.version != index as i64 + 1 {
            return Err(StoreError::Migration(format!("{app}: migration {} ({}) is at position {}; versions must be 1, 2, 3, ... with no gaps", migration.version, migration.name, index + 1)));
        }
    }
    let mut applied = 0;
    store.transaction(&mut |tx| {
        applied = 0;
        if store.engine() == Engine::Postgres {
            // One migrator at a time across every program on this database.
            tx.execute("SELECT pg_advisory_xact_lock(7310158203)", &[])?;
        }
        tx.execute(HISTORY_TABLE, &[])?;
        let done = history(tx, app)?;
        for (version, name) in &done {
            match migrations.get(*version as usize - 1) {
                Some(known) if known.name == name => {}
                Some(known) => return Err(StoreError::Migration(format!("{app}: migration {version} was applied as '{name}' but the code calls it '{}'", known.name))),
                None => return Err(StoreError::Migration(format!("{app}: the database is at migration {version} ('{name}'), newer than this program knows; upgrade the program"))),
            }
        }
        for migration in &migrations[done.len()..] {
            for statement in statements(migration.sql) {
                tx.execute(statement, &[])?;
            }
            tx.execute("INSERT INTO ortus_migrations (app, version, name, applied_at) VALUES ($1, $2, $3, $4)", &[app.into(), migration.version.into(), migration.name.into(), now_ms.into()])?;
            applied += 1;
        }
        Ok(())
    })?;
    Ok(applied)
}

/// The versions of `app` already applied, oldest first.
pub fn applied_versions(store: &dyn Store, app: &str) -> StoreResult<Vec<i64>> {
    let mut versions = Vec::new();
    store.transaction(&mut |tx| {
        tx.execute(HISTORY_TABLE, &[])?;
        versions = history(tx, app)?
            .into_iter()
            .map(|(version, _)| version)
            .collect();
        Ok(())
    })?;
    Ok(versions)
}

fn history(tx: &mut dyn Tx, app: &str) -> StoreResult<Vec<(i64, String)>> {
    let rows = tx.query(
        "SELECT version, name FROM ortus_migrations WHERE app = $1 ORDER BY version",
        &[Value::from(app)],
    )?;
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
