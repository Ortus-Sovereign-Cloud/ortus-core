//! Fresh databases of each engine for the test suites.
//!
//! SQLite always runs. PostgreSQL runs when `ORTUS_TEST_POSTGRES` holds a
//! connection URL; CI sets `ORTUS_REQUIRE_POSTGRES=1` so that a missing
//! database is a failure there, not a silent skip.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ortus_core::storage::blocking::on;
use ortus_core::storage::{AsyncStore, PostgresStore, SqliteStore};

/// Opens fresh, empty databases of one engine. Every call to `open`
/// returns a new connection to the *same* database.
pub struct Fresh {
    pub open: Box<dyn Fn() -> Arc<dyn AsyncStore> + Send + Sync>,
    _dir: Option<tempdir::Dir>,
}

pub mod tempdir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    pub struct Dir(pub PathBuf);

    impl Dir {
        pub fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ortus-core-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

pub fn sqlite() -> Option<Fresh> {
    let dir = tempdir::Dir::new();
    let file = dir.0.join("ortus.db");
    Some(Fresh {
        open: Box::new(move || Arc::new(SqliteStore::open(&file).unwrap())),
        _dir: Some(dir),
    })
}

pub fn postgres() -> Option<Fresh> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let Ok(url) = std::env::var("ORTUS_TEST_POSTGRES") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_POSTGRES").is_err(),
            "ORTUS_REQUIRE_POSTGRES is set but ORTUS_TEST_POSTGRES is not"
        );
        eprintln!("skipped: set ORTUS_TEST_POSTGRES to run this test on PostgreSQL");
        return None;
    };
    // Each test gets its own schema, so tests can run side by side.
    let schema = format!(
        "t{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    on(&PostgresStore::connect(&url, 1).unwrap())
        .execute(&format!("CREATE SCHEMA {schema}"), &[])
        .unwrap();
    let separator = if url.contains('?') { '&' } else { '?' };
    let scoped = format!("{url}{separator}options=-c%20search_path%3D{schema}");
    Some(Fresh {
        open: Box::new(move || Arc::new(PostgresStore::connect(&scoped, 4).unwrap())),
        _dir: None,
    })
}

/// A fresh, empty S3 file store under its own prefix, when S3 is set up.
pub fn s3_files() -> Option<ortus_core::files::blocking::Files> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let Ok(endpoint) = std::env::var("ORTUS_TEST_S3_ENDPOINT") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_S3").is_err(),
            "ORTUS_REQUIRE_S3 is set but ORTUS_TEST_S3_ENDPOINT is not"
        );
        eprintln!("skipped: set ORTUS_TEST_S3_ENDPOINT to run this test on S3");
        return None;
    };
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let settings = ortus_core::files::S3Settings {
        endpoint,
        bucket: var("ORTUS_TEST_S3_BUCKET"),
        region: std::env::var("ORTUS_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
        access_key: var("ORTUS_TEST_S3_ACCESS_KEY"),
        secret_key: var("ORTUS_TEST_S3_SECRET_KEY"),
        root: format!(
            "ortus-core-common-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
    };
    Some(ortus_core::files::blocking::Files::s3(&settings).unwrap())
}

/// A fresh, empty directory that is removed when dropped.
pub fn temp_dir() -> tempdir::Dir {
    tempdir::Dir::new()
}
