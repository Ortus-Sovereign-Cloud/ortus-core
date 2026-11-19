//! One test suite for the file interface, run against a local directory
//! and against an S3 store.
//!
//! The local directory always runs. S3 runs when `ORTUS_TEST_S3_ENDPOINT`
//! is set (with `ORTUS_TEST_S3_BUCKET`, `ORTUS_TEST_S3_ACCESS_KEY` and
//! `ORTUS_TEST_S3_SECRET_KEY`); CI sets `ORTUS_REQUIRE_S3=1` so that a
//! missing store is a failure there, not a silent skip.

use std::sync::atomic::{AtomicU32, Ordering};

use ortus_core::files::blocking::Files;
use ortus_core::files::{FilesError, S3Settings};

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Fresh {
    files: Files,
    dir: Option<std::path::PathBuf>,
}

impl Drop for Fresh {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn unique() -> String {
    format!(
        "ortus-core-files-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

fn local() -> Option<Fresh> {
    let dir = std::env::temp_dir().join(unique());
    Some(Fresh {
        files: Files::local(&dir).unwrap(),
        dir: Some(dir),
    })
}

fn s3() -> Option<Fresh> {
    let Ok(endpoint) = std::env::var("ORTUS_TEST_S3_ENDPOINT") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_S3").is_err(),
            "ORTUS_REQUIRE_S3 is set but ORTUS_TEST_S3_ENDPOINT is not"
        );
        eprintln!("skipped: set ORTUS_TEST_S3_ENDPOINT to run this test on S3");
        return None;
    };
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    // Each test works under its own prefix, so tests can run side by side.
    let settings = S3Settings {
        endpoint,
        bucket: var("ORTUS_TEST_S3_BUCKET"),
        region: std::env::var("ORTUS_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
        access_key: var("ORTUS_TEST_S3_ACCESS_KEY"),
        secret_key: var("ORTUS_TEST_S3_SECRET_KEY"),
        root: unique(),
    };
    Some(Fresh {
        files: Files::s3(&settings).unwrap(),
        dir: None,
    })
}

macro_rules! on_both_stores {
    ($($name:ident),* $(,)?) => {
        mod on_local { $( #[test] fn $name() { if let Some(store) = super::local() { super::$name(&store.files) } } )* }
        mod on_s3 { $( #[test] fn $name() { if let Some(store) = super::s3() { super::$name(&store.files) } } )* }
    };
}

on_both_stores!(
    async_calls_work_across_runtimes,
    contents_round_trip,
    missing_files_are_not_errors,
    put_replaces,
    list_returns_files_under_a_prefix,
    bad_keys_are_refused,
    stores_do_not_see_each_other
);

fn contents_round_trip(files: &Files) {
    let binary: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
    files.put("drive/ab/cdef", &binary).unwrap();
    files.put("empty", b"").unwrap();
    files
        .put("mail/ünïcode name.eml", "текст".as_bytes())
        .unwrap();
    assert_eq!(Some(binary), files.get("drive/ab/cdef").unwrap());
    assert_eq!(Some(Vec::new()), files.get("empty").unwrap());
    assert_eq!(
        Some("текст".as_bytes().to_vec()),
        files.get("mail/ünïcode name.eml").unwrap()
    );
    assert!(files.exists("drive/ab/cdef").unwrap());
}

fn missing_files_are_not_errors(files: &Files) {
    assert_eq!(None, files.get("nothing/here").unwrap());
    assert!(!files.exists("nothing/here").unwrap());
    files.delete("nothing/here").unwrap();
    assert!(files.list("nothing/").unwrap().is_empty());
    files.put("a/b", b"1").unwrap();
    files.delete("a/b").unwrap();
    assert_eq!(None, files.get("a/b").unwrap());
    assert!(!files.exists("a/b").unwrap());
}

fn put_replaces(files: &Files) {
    files.put("doc", b"first version, longer").unwrap();
    files.put("doc", b"second").unwrap();
    assert_eq!(Some(b"second".to_vec()), files.get("doc").unwrap());
}

fn list_returns_files_under_a_prefix(files: &Files) {
    for key in ["mail/1", "mail/sub/2", "mailbox/3", "drive/4"] {
        files.put(key, b"x").unwrap();
    }
    assert_eq!(vec!["mail/1", "mail/sub/2"], files.list("mail/").unwrap());
    assert_eq!(
        vec!["drive/4", "mail/1", "mail/sub/2", "mailbox/3"],
        files.list("").unwrap()
    );
}

fn bad_keys_are_refused(files: &Files) {
    for key in ["", "/abs", "a/../../escape", "a/", "a//b"] {
        assert!(
            matches!(files.put(key, b"x"), Err(FilesError::BadKey(_))),
            "{key:?}"
        );
        assert!(
            matches!(files.get(key), Err(FilesError::BadKey(_))),
            "{key:?}"
        );
    }
    assert!(
        files.list("").unwrap().is_empty(),
        "a refused key must leave nothing behind"
    );
}

fn stores_do_not_see_each_other(files: &Files) {
    // Every test has its own root; a file written by another test with
    // the same key must not show up here.
    assert_eq!(None, files.get("doc").unwrap());
    assert!(files.list("").unwrap().is_empty());
}

fn async_calls_work_across_runtimes(files: &Files) {
    let runtime = || {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    };
    let files = files.as_async();
    let first = runtime();
    first.block_on(async {
        files.put("a/one", b"1").await.unwrap();
        assert_eq!(Some(b"1".to_vec()), files.get("a/one").await.unwrap());
    });
    drop(first);
    // The caller's runtime is gone; the store's connections must not be.
    runtime().block_on(async {
        files.put("a/two", b"2").await.unwrap();
        assert!(files.exists("a/one").await.unwrap());
        assert_eq!(vec!["a/one", "a/two"], files.list("a/").await.unwrap());
        files.delete("a/one").await.unwrap();
        assert_eq!(None, files.get("a/one").await.unwrap());
    });
}
