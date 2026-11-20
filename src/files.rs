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
//!
//! The interface is async. Code that is not async uses [`blocking::Files`].

use std::fmt;
use std::path::Path;

use opendal::{options, services, ErrorKind, Operator};

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
    operator: Operator,
}

impl Files {
    /// A store in the local directory `root`, created when missing.
    pub fn local(root: &Path) -> FilesResult<Self> {
        let builder = services::Fs::default().root(&root.to_string_lossy());
        Ok(Self {
            operator: Operator::new(builder).map_err(error)?,
        })
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
        let operator = Operator::new(builder).map_err(error)?;
        let context = operator.context().with_http_transport(s3_transport()?);
        Ok(Self {
            operator: operator.with_context(context),
        })
    }

    /// Stores `contents` under `key`, replacing what was there.
    pub async fn put(&self, key: &str, contents: &[u8]) -> FilesResult<()> {
        let (operator, key, contents) = (
            self.operator.clone(),
            check(key)?.to_string(),
            contents.to_vec(),
        );
        on_io(async move { operator.write(&key, contents).await.map(|_| ()) }).await
    }

    /// The contents under `key`, or `None` when there is no such file.
    pub async fn get(&self, key: &str) -> FilesResult<Option<Vec<u8>>> {
        let (operator, key) = (self.operator.clone(), check(key)?.to_string());
        on_io(async move {
            match operator.read(&key).await {
                Ok(buffer) => Ok(Some(buffer.to_vec())),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
    }

    /// Whether a file exists under `key`.
    pub async fn exists(&self, key: &str) -> FilesResult<bool> {
        let (operator, key) = (self.operator.clone(), check(key)?.to_string());
        on_io(async move { operator.exists(&key).await }).await
    }

    /// Removes the file under `key`. Removing a missing file is not an error.
    pub async fn delete(&self, key: &str) -> FilesResult<()> {
        let (operator, key) = (self.operator.clone(), check(key)?.to_string());
        on_io(async move { operator.delete(&key).await }).await
    }

    /// The keys of every file under `prefix` (a key prefix ending in `/`,
    /// or empty for all files), sorted.
    pub async fn list(&self, prefix: &str) -> FilesResult<Vec<String>> {
        if !prefix.is_empty() {
            check(prefix.trim_end_matches('/'))?;
        }
        let (operator, prefix) = (self.operator.clone(), prefix.to_string());
        let entries = on_io(async move {
            match operator
                .list_options(
                    &prefix,
                    options::ListOptions {
                        recursive: true,
                        ..Default::default()
                    },
                )
                .await
            {
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
                other => other,
            }
        })
        .await?;
        let mut keys: Vec<String> = entries
            .into_iter()
            .filter(|entry| entry.metadata().is_file())
            .map(|entry| entry.path().to_string())
            .collect();
        keys.sort();
        Ok(keys)
    }
}

/// The HTTP client for S3. It trusts the public certificate authorities
/// built into the program (the webpki list), not the system's store: the
/// container image has no system store at all, and the program then works
/// the same everywhere.
fn s3_transport() -> FilesResult<opendal::HttpTransporter> {
    use std::sync::{Arc, OnceLock};
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
            reqwest::Client::builder()
                .tls_backend_preconfigured(tls)
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(|e| FilesError::Store(format!("cannot set up the S3 client: {e}")))?;
    Ok(opendal::HttpTransporter::new(
        opendal_http_transport_reqwest::ReqwestTransport::new(client),
    ))
}

/// Runs a store operation on the process-wide I/O runtime, whichever
/// runtime the caller is on. The HTTP client behind S3 is shared by the
/// whole process and keeps its open connections on the runtime that made
/// them; were that a caller's own runtime, the connections would break
/// when it shut down.
async fn on_io<T: Send + 'static>(
    work: impl std::future::Future<Output = opendal::Result<T>> + Send + 'static,
) -> FilesResult<T> {
    crate::runtime::io()
        .spawn(work)
        .await
        .map_err(|e| FilesError::Store(format!("the file operation was cancelled: {e}")))?
        .map_err(error)
}

/// The file interface for code that is not async. Do not call these from
/// async code.
pub mod blocking {
    use std::path::Path;

    use super::{FilesResult, S3Settings};
    use crate::runtime::block_on;

    /// A place to keep file contents, with calls that wait for the result.
    pub struct Files {
        inner: super::Files,
    }

    impl From<super::Files> for Files {
        fn from(inner: super::Files) -> Self {
            Self { inner }
        }
    }

    impl Files {
        /// See [`Files::local`](super::Files::local).
        pub fn local(root: &Path) -> FilesResult<Self> {
            super::Files::local(root).map(Self::from)
        }

        /// See [`Files::s3`](super::Files::s3).
        pub fn s3(settings: &S3Settings) -> FilesResult<Self> {
            super::Files::s3(settings).map(Self::from)
        }

        /// The async store underneath.
        pub fn as_async(&self) -> &super::Files {
            &self.inner
        }

        pub fn put(&self, key: &str, contents: &[u8]) -> FilesResult<()> {
            block_on(self.inner.put(key, contents))
        }

        pub fn get(&self, key: &str) -> FilesResult<Option<Vec<u8>>> {
            block_on(self.inner.get(key))
        }

        pub fn exists(&self, key: &str) -> FilesResult<bool> {
            block_on(self.inner.exists(key))
        }

        pub fn delete(&self, key: &str) -> FilesResult<()> {
            block_on(self.inner.delete(key))
        }

        pub fn list(&self, prefix: &str) -> FilesResult<Vec<String>> {
            block_on(self.inner.list(prefix))
        }
    }
}

/// Refuses keys that could leave the store's root or that the two kinds of
/// store would treat differently.
pub(crate) fn check(key: &str) -> FilesResult<&str> {
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
