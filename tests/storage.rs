//! One test suite for the storage interface, run against every engine.
//!
//! SQLite always runs. PostgreSQL runs when `ORTUS_TEST_POSTGRES` holds a
//! connection URL; CI sets `ORTUS_REQUIRE_POSTGRES=1` so that a missing
//! database is a failure there, not a silent skip.

mod common;

use std::sync::Arc;

use common::{postgres, sqlite, Fresh};
use ortus_core::storage::blocking::{applied_versions, migrate, on};
use ortus_core::storage::{AsyncStore, Engine, Migration, StoreError, Value};

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
    a_commit_after_a_failed_statement_fails,
    applied_migrations_keep_their_checksums,
    a_changed_table_is_read_as_it_is_now,
    in_lists_of_any_length,
    a_transaction_that_does_nothing_ends_cleanly,
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
    let changed = on(&*store)
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
    let rows = on(&*store)
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
    assert!(on(&*store)
        .query("SELECT id FROM notes WHERE id = $1", &[8.into()])
        .unwrap()
        .is_empty());
}

fn duplicate_key_is_a_conflict(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    on(&*store)
        .execute(
            "INSERT INTO notes (id, title) VALUES ($1, $2)",
            &[1.into(), "a".into()],
        )
        .unwrap();
    let again = on(&*store).execute(
        "INSERT INTO notes (id, title) VALUES ($1, $2)",
        &[1.into(), "b".into()],
    );
    assert!(
        matches!(again, Err(StoreError::Conflict(_))),
        "got {again:?}"
    );
    // The connection is still usable after the failure.
    assert_eq!(
        1,
        on(&*store)
            .query("SELECT id FROM notes", &[])
            .unwrap()
            .len()
    );
}

fn failed_transaction_changes_nothing(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    let result = on(&*store).transaction(&mut |tx| {
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
    assert!(on(&*store)
        .query("SELECT id FROM notes", &[])
        .unwrap()
        .is_empty());
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
        on(&*store).query("SELECT name FROM tags", &[]).unwrap()[0].text(0)
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
        on(&*store).query("SELECT id FROM notes", &[]).is_err(),
        "migration 1 must be rolled back with migration 2"
    );
    assert!(on(&*store).query("SELECT id FROM half", &[]).is_err());
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
    let stores: Vec<Arc<dyn AsyncStore>> = (0..4).map(|_| (db.open)()).collect();
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
                on(&*store)
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
    assert_eq!(
        16,
        on(&*store)
            .query("SELECT id FROM notes", &[])
            .unwrap()
            .len()
    );
    assert!(matches!(store.engine(), Engine::Sqlite | Engine::Postgres));
}

/// Code that catches an error inside a transaction and commits anyway gets
/// an error on both engines, and nothing of the transaction is kept.
fn a_commit_after_a_failed_statement_fails(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    on(&*store)
        .execute("INSERT INTO notes (id, title) VALUES (1, 'a')", &[])
        .unwrap();
    ortus_core::runtime::block_on(async {
        let mut tx = store.begin().await.unwrap();
        tx.execute("INSERT INTO notes (id, title) VALUES (2, 'b')", &[])
            .await
            .unwrap();
        let again = tx
            .execute("INSERT INTO notes (id, title) VALUES (1, 'c')", &[])
            .await;
        assert!(matches!(again, Err(StoreError::Conflict(_))), "{again:?}");
        assert!(tx.commit().await.is_err());
    });
    let ids: Vec<i64> = on(&*store)
        .query("SELECT id FROM notes ORDER BY id", &[])
        .unwrap()
        .iter()
        .filter_map(|r| r.int(0))
        .collect();
    assert_eq!(vec![1], ids);
}

/// Each applied migration's SQL is remembered by its SHA-256 (older
/// databases get theirs on the next start); a migration changed after it
/// was applied is only reported, never a reason not to start.
fn applied_migrations_keep_their_checksums(db: &Fresh) {
    let store = (db.open)();
    assert_eq!(1, migrate(store.as_ref(), "sums", &[NOTES], 10).unwrap());
    let sums = |store: &dyn AsyncStore| -> Vec<(i64, String)> {
        on(store)
            .query("SELECT version, checksum FROM ortus_migration_sums WHERE app = 'sums' ORDER BY version", &[])
            .unwrap()
            .iter()
            .map(|r| (r.int(0).unwrap(), r.text(1).unwrap().to_string()))
            .collect()
    };
    let first = sums(store.as_ref());
    assert_eq!(1, first.len());
    assert_eq!(64, first[0].1.len(), "a SHA-256 in hex");
    // A database from before the sums: they are filled in.
    on(&*store)
        .execute("DELETE FROM ortus_migration_sums", &[])
        .unwrap();
    assert_eq!(0, migrate(store.as_ref(), "sums", &[NOTES], 20).unwrap());
    assert_eq!(first, sums(store.as_ref()));
    // Changed afterwards: reported, and the program still starts.
    let changed = Migration {
        version: 1,
        name: "notes",
        sql: "CREATE TABLE notes (id BIGINT PRIMARY KEY, title TEXT)",
    };
    assert_eq!(0, migrate(store.as_ref(), "sums", &[changed], 30).unwrap());
    assert_eq!(
        first,
        sums(store.as_ref()),
        "the applied migration's sum stays"
    );
}

/// Statements are kept prepared; a table changed afterwards, here or by
/// another program, is still read as it is now (another program's change
/// may fail one statement on each connection, never more).
fn a_changed_table_is_read_as_it_is_now(db: &Fresh) {
    let (store, other) = ((db.open)(), (db.open)());
    // PostgreSQL's prepared statements (SQLite keeps the width a cached
    // `SELECT *` had; Ortus names its columns).
    if store.engine() != Engine::Postgres {
        return;
    }
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    on(&*store)
        .execute("INSERT INTO notes (id, title) VALUES (1, 'a')", &[])
        .unwrap();
    let width = |store: &dyn AsyncStore| -> Result<usize, StoreError> {
        Ok(on(store).query("SELECT * FROM notes", &[])?[0].0.len())
    };
    assert_eq!(4, width(store.as_ref()).unwrap());
    on(&*store)
        .execute("ALTER TABLE notes ADD COLUMN extra TEXT", &[])
        .unwrap();
    assert_eq!(5, width(store.as_ref()).unwrap());
    on(&*other)
        .execute("ALTER TABLE notes ADD COLUMN more TEXT", &[])
        .unwrap();
    let now = width(store.as_ref()).or_else(|_| width(store.as_ref()));
    assert_eq!(6, now.unwrap());
}

fn in_lists_of_any_length(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    for id in 0..10i64 {
        on(&*store)
            .execute(
                "INSERT INTO notes (id, title) VALUES ($1, 'n')",
                &[id.into()],
            )
            .unwrap();
    }
    for wanted in [
        vec![],
        vec![3i64],
        vec![1, 2, 9],
        (0..7).collect(),
        (0..40).collect(),
    ] {
        let (list, mut values) = ortus_core::storage::any_of(2, &wanted);
        values.insert(0, "n".into());
        let ids: Vec<i64> = on(&*store)
            .query(
                &format!("SELECT id FROM notes WHERE title = $1 AND id IN ({list}) ORDER BY id"),
                &values,
            )
            .unwrap()
            .iter()
            .filter_map(|r| r.int(0))
            .collect();
        let expected: Vec<i64> = wanted.iter().copied().filter(|id| *id < 10).collect();
        assert_eq!(expected, ids);
    }
}

fn a_transaction_that_does_nothing_ends_cleanly(db: &Fresh) {
    let store = (db.open)();
    migrate(store.as_ref(), "test", &[NOTES], 1).unwrap();
    ortus_core::runtime::block_on(async {
        store.begin().await.unwrap().commit().await.unwrap();
        store.begin_read().await.unwrap().rollback().await.unwrap();
        // Rolled back after its first statement: nothing kept.
        let mut tx = store.begin().await.unwrap();
        tx.execute("INSERT INTO notes (id, title) VALUES (5, 'x')", &[])
            .await
            .unwrap();
        tx.rollback().await.unwrap();
    });
    assert!(on(&*store)
        .query("SELECT id FROM notes", &[])
        .unwrap()
        .is_empty());
}
