//! The file interface: where file contents live, on a disk or in S3.
//!
//! The single-machine program keeps files in a local directory; the
//! Kubernetes chart keeps them in an S3 store. Services use [`Files`] and
//! do not know which one they are on.
//!
//! Keys are paths with `/` separators, relative to the store's root, for
//! example `drive/ab/cdef`. A key names one file; there are no directories
//! to create or remove.
//!
//! Files are read and written whole. Streaming of large files is not part
//! of this interface yet.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use opendal::{blocking, options, services, ErrorKind, Operator};

/// A file-store failure.
#[derive(Debug)]
pub enum FilesError {
    /// The key is not a valid file name in the store.
    BadKey(String),
    /// Anything the store reported.
    Store(String),
}

impl fmt::Display for FilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilesError::BadKey(m) => write!(f, "bad file key: {m}"),
            FilesError::Store(m) => write!(f, "file store: {m}"),
        }
    }
}

impl std::error::Error for FilesError {}

pub type FilesResult<T> = Result<T, FilesError>;

/// Where an S3 store is and how to sign in to it.
#[derive(Clone, Debug)]
pub struct S3Settings {
    /// Base URL of the store, for example `http://rustfs:9000`.
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Prefix inside the bucket that this installation owns; empty for the
    /// whole bucket.
    pub root: String,
}

/// A place to keep file contents.
pub struct Files {
    operator: blocking::Operator,
}

/// The runtime OpenDAL does its I/O on while callers block. There is one
/// for the whole process and it is never shut down: the HTTP client behind
/// S3 is shared process-wide and keeps its open connections on this
/// runtime, so a runtime per store would break them when a store is dropped.
fn runtime() -> FilesResult<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("ortus-files")
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| FilesError::Store(format!("could not start the file runtime: {e}")))
}

impl Files {
    /// A store in the local directory `root`, created when missing.
    pub fn local(root: &Path) -> FilesResult<Self> {
        let builder = services::Fs::default().root(&root.to_string_lossy());
        Self::build(Operator::new(builder).map_err(error)?)
    }

    /// A store in an S3 bucket. The bucket must exist.
    pub fn s3(settings: &S3Settings) -> FilesResult<Self> {
        let builder = services::S3::default()
            .endpoint(&settings.endpoint)
            .bucket(&settings.bucket)
            .region(&settings.region)
            .access_key_id(&settings.access_key)
            .secret_access_key(&settings.secret_key)
            .root(&format!("/{}", settings.root.trim_matches('/')));
        Self::build(Operator::new(builder).map_err(error)?)
    }

    fn build(operator: Operator) -> FilesResult<Self> {
        let _context = runtime()?.enter();
        Ok(Self {
            operator: blocking::Operator::new(operator).map_err(error)?,
        })
    }

    /// Stores `contents` under `key`, replacing what was there.
    pub fn put(&self, key: &str, contents: &[u8]) -> FilesResult<()> {
        self.operator
            .write(check(key)?, contents.to_vec())
            .map(|_| ())
            .map_err(error)
    }

    /// The contents under `key`, or `None` when there is no such file.
    pub fn get(&self, key: &str) -> FilesResult<Option<Vec<u8>>> {
        match self.operator.read(check(key)?) {
            Ok(buffer) => Ok(Some(buffer.to_vec())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(error(e)),
        }
    }

    /// Whether a file exists under `key`.
    pub fn exists(&self, key: &str) -> FilesResult<bool> {
        self.operator.exists(check(key)?).map_err(error)
    }

    /// Removes the file under `key`. Removing a missing file is not an error.
    pub fn delete(&self, key: &str) -> FilesResult<()> {
        self.operator.delete(check(key)?).map_err(error)
    }

    /// The keys of every file under `prefix` (a key prefix ending in `/`,
    /// or empty for all files), sorted.
    pub fn list(&self, prefix: &str) -> FilesResult<Vec<String>> {
        if !prefix.is_empty() {
            check(prefix.trim_end_matches('/'))?;
        }
        let entries = match self.operator.list_options(
            prefix,
            options::ListOptions {
                recursive: true,
                ..Default::default()
            },
        ) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(error(e)),
        };
        let mut keys: Vec<String> = entries
            .into_iter()
            .filter(|entry| entry.metadata().is_file())
            .map(|entry| entry.path().to_string())
            .collect();
        keys.sort();
        Ok(keys)
    }
}

/// Refuses keys that could leave the store's root or that the two kinds of
/// store would treat differently.
fn check(key: &str) -> FilesResult<&str> {
    let bad = |why: &str| Err(FilesError::BadKey(format!("'{key}': {why}")));
    if key.is_empty() {
        return bad("empty");
    }
    if key.starts_with('/') || key.ends_with('/') {
        return bad("must not start or end with '/'");
    }
    if key
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return bad("empty, '.' and '..' path parts are not allowed");
    }
    if key.chars().any(|c| c.is_control() || c == '\\') {
        return bad("control characters and '\\' are not allowed");
    }
    Ok(key)
}

fn error(e: opendal::Error) -> FilesError {
    FilesError::Store(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn keys_that_escape_the_root_are_refused() {
        for key in [
            "",
            "/etc/passwd",
            "a/../b",
            "..",
            "a//b",
            "a/",
            "./a",
            "a\\b",
            "a\nb",
        ] {
            assert!(check(key).is_err(), "{key:?} must be refused");
        }
        for key in [
            "a",
            "drive/ab/cdef",
            "mail/2026/11/message.eml",
            "name with space.txt",
            "ünïcode/файл",
        ] {
            assert!(check(key).is_ok(), "{key:?} must be accepted");
        }
    }
}
