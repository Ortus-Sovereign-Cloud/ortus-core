//! One test suite for the storage interface, run against every engine.
//!
//! SQLite always runs. PostgreSQL runs when `ORTUS_TEST_POSTGRES` holds a
//! connection URL; CI sets `ORTUS_REQUIRE_POSTGRES=1` so that a missing
//! database is a failure there, not a silent skip.

mod common;

use std::sync::Arc;

use common::{postgres, sqlite, Fresh};
use ortus_core::storage::{applied_versions, migrate, Engine, Migration, Store, StoreError, Value};

/// Declares one test per engine for a function that takes a fresh database.
macro_rules! on_both_engines {
    ($($name:ident),* $(,)?) => {
        mod on_sqlite { $( #[test] fn $name() { if let Some(db) = super::sqlite() { super::$name(&db) } } )* }
        mod on_postgres { $( #[test] fn $name() { if let Some(db) = super::postgres() { super::$name(&db) } } )* }
    };
}

on_both_engines!(
    values_round_trip,
    duplicate_key_is_a_conflict,
    failed_transaction_changes_nothing,
    migrations_apply_once_and_in_order,
    failed_migration_leaves_the_database_as_it_was,
    wrong_migration_lists_are_refused,
    programs_starting_together_migrate_once,
    more_transactions_than_connections_all_finish,
);

const NOTES: Migration = Migration { version: 1, name: "notes", sql: "CREATE TABLE notes (id BIGINT PRIMARY KEY, title TEXT NOT NULL, body {{blob}}, pinned BIGINT)" };
const TAGS: Migration = Migration {
    version: 2,
    name: "tags",
    sql: "CREATE TABLE tags (name TEXT PRIMARY KEY); INSERT INTO tags (name) VALUES ('first; tag')",
};

fn values_round_trip(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    let body = vec![0u8, 255, 10, 0];
    let changed = store
        .execute(
            "INSERT INTO notes (id, title, body, pinned) VALUES ($1, $2, $3, $4)",
            &[
                7.into(),
                "costs $1 — it's fine".into(),
                body.clone().into(),
                Value::Null,
            ],
        )
        .unwrap();
    assert_eq!(1, changed);
    let rows = store
        .query(
            "SELECT id, title, body, pinned, '$1 literal' FROM notes WHERE id = $1",
            &[7.into()],
        )
        .unwrap();
    assert_eq!(1, rows.len());
    assert_eq!(Some(7), rows[0].int(0));
    assert_eq!(Some("costs $1 — it's fine"), rows[0].text(1));
    assert_eq!(Some(body.as_slice()), rows[0].blob(2));
    assert!(rows[0].get(3).is_null());
    assert_eq!(Some("$1 literal"), rows[0].text(4));
    assert!(store
        .query("SELECT id FROM notes WHERE id = $1", &[8.into()])
        .unwrap()
        .is_empty());
}

fn duplicate_key_is_a_conflict(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    store
        .execute(
            "INSERT INTO notes (id, title) VALUES ($1, $2)",
            &[1.into(), "a".into()],
        )
        .unwrap();
    let again = store.execute(
        "INSERT INTO notes (id, title) VALUES ($1, $2)",
        &[1.into(), "b".into()],
    );
    assert!(
        matches!(again, Err(StoreError::Conflict(_))),
        "got {again:?}"
    );
    // The connection is still usable after the failure.
    assert_eq!(1, store.query("SELECT id FROM notes", &[]).unwrap().len());
}

fn failed_transaction_changes_nothing(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    let result = store.transaction(&mut |tx| {
        tx.execute(
            "INSERT INTO notes (id, title) VALUES ($1, $2)",
            &[1.into(), "kept?".into()],
        )?;
        assert_eq!(
            1,
            tx.query("SELECT id FROM notes", &[])?.len(),
            "a transaction sees its own writes"
        );
        Err(StoreError::Database("stop".into()))
    });
    assert!(result.is_err());
    assert!(store.query("SELECT id FROM notes", &[]).unwrap().is_empty());
}

fn migrations_apply_once_and_in_order(db: &Fresh) {
    let store = (db.open)();
    assert!(applied_versions(store.as_ref(), "test").unwrap().is_empty());
    assert_eq!(1, migrate(store.as_ref(), "test", &[NOTES], 10).unwrap());
    assert_eq!(0, migrate(store.as_ref(), "test", &[NOTES], 20).unwrap());
    assert_eq!(
        1,
        migrate(store.as_ref(), "test", &[NOTES, TAGS], 30).unwrap()
    );
    assert_eq!(
        vec![1, 2],
        applied_versions(store.as_ref(), "test").unwrap()
    );
    // A `;` inside a literal does not end the statement.
    assert_eq!(
        Some("first; tag"),
        store.query("SELECT name FROM tags", &[]).unwrap()[0].text(0)
    );
    // Another app has its own history in the same database.
    assert_eq!(
        1,
        migrate(
            store.as_ref(),
            "other",
            &[Migration {
                version: 1,
                name: "things",
                sql: "CREATE TABLE things (id BIGINT PRIMARY KEY)"
            }],
            40
        )
        .unwrap()
    );
    assert_eq!(vec![1], applied_versions(store.as_ref(), "other").unwrap());
}

fn failed_migration_leaves_the_database_as_it_was(db: &Fresh) {
    let store = (db.open)();
    let broken = Migration {
        version: 2,
        name: "broken",
        sql: "CREATE TABLE half (id BIGINT PRIMARY KEY); THIS IS NOT SQL",
    };
    assert!(migrate(store.as_ref(), "test", &[NOTES, broken], 1).is_err());
    assert!(applied_versions(store.as_ref(), "test").unwrap().is_empty());
    assert!(
        store.query("SELECT id FROM notes", &[]).is_err(),
        "migration 1 must be rolled back with migration 2"
    );
    assert!(store.query("SELECT id FROM half", &[]).is_err());
    // After the fix the same database migrates cleanly.
    assert_eq!(1, migrate(store.as_ref(), "test", &[NOTES], 2).unwrap());
}

fn wrong_migration_lists_are_refused(db: &Fresh) {
    let store = (db.open)();
    let gap = Migration {
        version: 3,
        name: "gap",
        sql: "CREATE TABLE gap (id BIGINT PRIMARY KEY)",
    };
    assert!(matches!(
        migrate(store.as_ref(), "test", &[NOTES, gap], 1),
        Err(StoreError::Migration(_))
    ));
    assert_eq!(1, migrate(store.as_ref(), "test", &[NOTES], 1).unwrap());
    let renamed = Migration {
        name: "renamed",
        ..NOTES
    };
    assert!(matches!(
        migrate(store.as_ref(), "test", &[renamed], 2),
        Err(StoreError::Migration(_))
    ));
    // A program older than the database must not run against it.
    assert!(matches!(
        migrate(store.as_ref(), "test", &[], 3),
        Err(StoreError::Migration(_))
    ));
}

fn programs_starting_together_migrate_once(db: &Fresh) {
    let stores: Vec<Arc<dyn Store>> = (0..4).map(|_| (db.open)()).collect();
    let handles: Vec<_> = stores
        .into_iter()
        .map(|store| {
            std::thread::spawn(move || migrate(store.as_ref(), "test", &[NOTES], 1).unwrap())
        })
        .collect();
    let applied: usize = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .sum();
    assert_eq!(1, applied, "exactly one program applies the migration");
    assert_eq!(
        vec![1],
        applied_versions((db.open)().as_ref(), "test").unwrap()
    );
}

fn more_transactions_than_connections_all_finish(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    let handles: Vec<_> = (0..16i64)
        .map(|id| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                store
                    .execute(
                        "INSERT INTO notes (id, title) VALUES ($1, $2)",
                        &[id.into(), "n".into()],
                    )
                    .unwrap()
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(1, handle.join().unwrap());
    }
    assert_eq!(16, store.query("SELECT id FROM notes", &[]).unwrap().len());
    assert!(matches!(store.engine(), Engine::Sqlite | Engine::Postgres));
}
