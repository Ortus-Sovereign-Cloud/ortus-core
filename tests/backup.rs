//! Backup and restore between every pair of shapes: SQLite or PostgreSQL,
//! a local directory or S3.

mod common;

use std::sync::Arc;

use ortus_core::backup::{self, BackupError};
use ortus_core::files::blocking::Files;
use ortus_core::runtime::block_on;
use ortus_core::storage::{AsyncStore, Migration, Schema, Value};

const NOTES: &[Migration] = &[
    Migration {
        version: 1,
        name: "notes",
        sql: "CREATE TABLE test_folders (id TEXT PRIMARY KEY, name TEXT NOT NULL);
              CREATE TABLE test_notes (id BIGINT PRIMARY KEY, folder TEXT NOT NULL REFERENCES test_folders (id), body TEXT, data {{blob}});
              CREATE TABLE test_replies (id BIGINT PRIMARY KEY, parent BIGINT REFERENCES test_replies (id) DEFERRABLE INITIALLY DEFERRED)",
    },
    Migration {
        version: 2,
        name: "note pins",
        sql: "ALTER TABLE test_notes ADD COLUMN pinned BIGINT NOT NULL DEFAULT 0",
    },
];

/// An app whose name sorts before the core's and whose migration touches a
/// core table: a restore must make the core's schemas first.
const EARLY: &[Migration] = &[Migration {
    version: 1,
    name: "touches the search index",
    sql: "UPDATE core_search SET at = at WHERE 1 = 0",
}];

fn schemas() -> Vec<Schema> {
    let mut all = ortus_core::schemas();
    all.push(Schema {
        app: "test.notes",
        migrations: NOTES,
    });
    all.push(Schema {
        app: "aaa",
        migrations: EARLY,
    });
    all
}

struct Shape {
    store: Arc<dyn AsyncStore>,
    files: Files,
    _keep: (Option<common::Fresh>, Option<common::tempdir::Dir>),
}

fn sqlite_local() -> Option<Shape> {
    let fresh = common::sqlite()?;
    let dir = common::temp_dir();
    Some(Shape {
        store: (fresh.open)(),
        files: Files::local(&dir.0.join("files")).unwrap(),
        _keep: (Some(fresh), Some(dir)),
    })
}

fn postgres_s3() -> Option<Shape> {
    let fresh = common::postgres()?;
    let files = common::s3_files()?;
    Some(Shape {
        store: (fresh.open)(),
        files,
        _keep: (Some(fresh), None),
    })
}

fn fill(shape: &Shape) {
    let store = shape.store.as_ref();
    block_on(async {
        ortus_core::storage::install(store, &schemas(), 1)
            .await
            .unwrap();
        store
            .execute(
                "INSERT INTO test_folders (id, name) VALUES ('f1', 'Flood response')",
                &[],
            )
            .await
            .unwrap();
        let binary: Vec<u8> = (0..=255u8).collect();
        for (id, body, data) in [
            (
                1i64,
                Value::from("Sandbags at the river, 'quoted' and \"double\""),
                Value::from(binary),
            ),
            (2, Value::Null, Value::Null),
            (
                3,
                Value::from("Ukrainian: повінь, Arabic: فيضان"),
                Value::from(Vec::<u8>::new()),
            ),
        ] {
            store
                .execute(
                    "INSERT INTO test_notes (id, folder, body, data, pinned) VALUES ($1, 'f1', $2, $3, 1)",
                    &[id.into(), body, data],
                )
                .await
                .unwrap();
        }
    });
    block_on(async {
        // A reply refers to one inserted after it: in the backup the child
        // can come first.
        let mut tx = store.begin().await.unwrap();
        tx.execute("INSERT INTO test_replies (id, parent) VALUES (1, 2)", &[])
            .await
            .unwrap();
        tx.execute(
            "INSERT INTO test_replies (id, parent) VALUES (2, NULL)",
            &[],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    });
    for n in 0..3 {
        block_on(ortus_core::audit::record(
            store,
            &ortus_core::audit::Event::new("test.event", ortus_core::audit::Outcome::Done)
                .by("u_test")
                .with(serde_json::json!({ "n": n })),
            10 + n,
        ))
        .unwrap();
    }
    shape.files.put("drive/ab/one", b"first file").unwrap();
    shape.files.put("mail/raw/2", &[0u8, 1, 2, 255]).unwrap();
    shape.files.put("empty", b"").unwrap();
}

fn notes(shape: &Shape) -> Vec<Vec<Value>> {
    block_on(shape.store.query(
        "SELECT id, folder, body, data, pinned FROM test_notes ORDER BY id",
        &[],
    ))
    .unwrap()
    .into_iter()
    .map(|row| row.0)
    .collect()
}

fn round_trip(from: Shape, to: Shape) {
    fill(&from);
    let dir = common::temp_dir();
    let target = dir.0.join("backup");
    let manifest = block_on(backup::backup(
        from.store.as_ref(),
        from.files.as_async(),
        &target,
        42,
    ))
    .unwrap();
    assert_eq!(manifest.schemas.get("test.notes"), Some(&2));
    assert_eq!(manifest.files.count, 3);

    let checked = backup::check(&target, &schemas()).unwrap();
    block_on(backup::restore(
        &checked,
        &schemas(),
        to.store.as_ref(),
        to.files.as_async(),
        43,
    ))
    .unwrap();

    assert_eq!(notes(&from), notes(&to));
    let replies = block_on(
        to.store
            .query("SELECT id, parent FROM test_replies ORDER BY id", &[]),
    )
    .unwrap();
    assert_eq!(2, replies.len());
    assert_eq!(3, notes(&to).len());
    assert_eq!(from.files.list("").unwrap(), to.files.list("").unwrap());
    assert_eq!(
        Some(vec![0u8, 1, 2, 255]),
        to.files.get("mail/raw/2").unwrap()
    );
    // The restored database starts like any other: nothing left to apply.
    block_on(ortus_core::storage::install(
        to.store.as_ref(),
        &schemas(),
        44,
    ))
    .unwrap();
    let applied = block_on(ortus_core::storage::applied_versions(
        to.store.as_ref(),
        "test.notes",
    ))
    .unwrap();
    assert_eq!(vec![1, 2], applied);
}

#[test]
fn sqlite_and_a_directory_restore_into_sqlite_and_a_directory() {
    round_trip(sqlite_local().unwrap(), sqlite_local().unwrap());
}

#[test]
fn sqlite_and_a_directory_restore_into_postgres_and_s3() {
    if let Some(to) = postgres_s3() {
        round_trip(sqlite_local().unwrap(), to);
    }
}

#[test]
fn postgres_and_s3_restore_into_sqlite_and_a_directory() {
    if let Some(from) = postgres_s3() {
        round_trip(from, sqlite_local().unwrap());
    }
}

#[test]
fn postgres_and_s3_restore_into_postgres_and_s3() {
    if let (Some(from), Some(to)) = (postgres_s3(), postgres_s3()) {
        round_trip(from, to);
    }
}

fn taken() -> (Shape, common::tempdir::Dir) {
    let from = sqlite_local().unwrap();
    fill(&from);
    let dir = common::temp_dir();
    block_on(backup::backup(
        from.store.as_ref(),
        from.files.as_async(),
        &dir.0.join("b"),
        1,
    ))
    .unwrap();
    (from, dir)
}

fn message(result: Result<impl Sized, BackupError>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.0,
    }
}

#[test]
fn a_backup_goes_into_a_new_directory_only() {
    let (from, dir) = taken();
    let again = block_on(backup::backup(
        from.store.as_ref(),
        from.files.as_async(),
        &dir.0.join("b"),
        2,
    ));
    assert!(message(again).contains("not empty"));
}

#[test]
fn restore_refuses_a_database_or_file_store_in_use() {
    let (_, dir) = taken();
    let checked = backup::check(&dir.0.join("b"), &schemas()).unwrap();

    let used = sqlite_local().unwrap();
    fill(&used);
    let result = block_on(backup::restore(
        &checked,
        &schemas(),
        used.store.as_ref(),
        used.files.as_async(),
        1,
    ));
    assert!(message(result).contains("database is not empty"));

    let files_used = sqlite_local().unwrap();
    files_used.files.put("left/over", b"x").unwrap();
    let result = block_on(backup::restore(
        &checked,
        &schemas(),
        files_used.store.as_ref(),
        files_used.files.as_async(),
        1,
    ));
    assert!(message(result).contains("file store is not empty"));
    // Nothing was written to the database either.
    let tables = block_on(files_used.store.query(
        "SELECT name FROM sqlite_master WHERE name = 'test_notes'",
        &[],
    ))
    .unwrap();
    assert!(tables.is_empty());
}

#[test]
fn a_damaged_backup_is_refused_before_anything_is_written() {
    let (_, dir) = taken();
    let b = dir.0.join("b");
    std::fs::write(b.join("files/drive/ab/one"), b"changed").unwrap();
    assert!(message(backup::check(&b, &schemas())).contains("damaged"));

    let (_, dir) = taken();
    let b = dir.0.join("b");
    let table = b.join("tables/test_notes.jsonl");
    let text = std::fs::read_to_string(&table)
        .unwrap()
        .replace("Sandbags", "Sandbag");
    std::fs::write(&table, text).unwrap();
    assert!(message(backup::check(&b, &schemas())).contains("damaged"));

    let (_, dir) = taken();
    let b = dir.0.join("b");
    std::fs::remove_file(b.join("manifest.json")).unwrap();
    assert!(message(backup::check(&b, &schemas())).contains("incomplete"));
}

#[test]
fn a_backup_from_a_newer_program_or_with_unknown_parts_is_refused() {
    let (_, dir) = taken();
    let b = dir.0.join("b");
    let mut older = ortus_core::schemas();
    older.push(Schema {
        app: "test.notes",
        migrations: &NOTES[..1],
    });
    older.push(Schema {
        app: "aaa",
        migrations: EARLY,
    });
    assert!(message(backup::check(&b, &older)).contains("newer than this program"));
    assert!(message(backup::check(&b, &ortus_core::schemas())).contains("does not have"));
}

#[test]
fn a_backup_restores_into_a_program_that_has_moved_on() {
    // The backup was taken at version 1 of test.notes; this program is at 2.
    let from = sqlite_local().unwrap();
    let old = vec![
        ortus_core::jobs::SCHEMA,
        Schema {
            app: "test.notes",
            migrations: &NOTES[..1],
        },
    ];
    block_on(async {
        ortus_core::storage::install(from.store.as_ref(), &old, 1)
            .await
            .unwrap();
        from.store
            .execute("INSERT INTO test_folders (id, name) VALUES ('f', 'n')", &[])
            .await
            .unwrap();
        from.store
            .execute(
                "INSERT INTO test_notes (id, folder, body) VALUES (7, 'f', 'old')",
                &[],
            )
            .await
            .unwrap();
    });
    let dir = common::temp_dir();
    let b = dir.0.join("b");
    block_on(backup::backup(
        from.store.as_ref(),
        from.files.as_async(),
        &b,
        1,
    ))
    .unwrap();

    let to = sqlite_local().unwrap();
    let checked = backup::check(&b, &schemas()).unwrap();
    block_on(backup::restore(
        &checked,
        &schemas(),
        to.store.as_ref(),
        to.files.as_async(),
        2,
    ))
    .unwrap();
    block_on(ortus_core::storage::install(
        to.store.as_ref(),
        &schemas(),
        3,
    ))
    .unwrap();
    let rows = block_on(
        to.store
            .query("SELECT id, body, pinned FROM test_notes", &[]),
    )
    .unwrap();
    assert_eq!(
        vec![Value::from(7i64), Value::from("old"), Value::from(0i64)],
        rows[0].0
    );
}

#[test]
fn table_names_in_a_backup_are_never_trusted() {
    let (_, dir) = taken();
    let b = dir.0.join("b");
    let path = b.join("manifest.json");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"test_folders\"", "\"test_folders; DROP TABLE x\"");
    std::fs::write(&path, text).unwrap();
    assert!(message(backup::check(&b, &schemas())).contains("not a table or column name"));
}
