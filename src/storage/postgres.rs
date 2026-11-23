//! The PostgreSQL engine, used by the Kubernetes chart.

use std::error::Error;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{to_sql_checked, IsNull, ToSql, Type};
use tokio_postgres::{Client, NoTls};

use super::{
    dialect, AsyncStore, AsyncTransaction, AsyncTx, BoxFuture, Engine, Row, StoreError,
    StoreResult, Value,
};
use crate::runtime;

/// A PostgreSQL database, reached through a small pool of connections.
pub struct PostgresStore {
    pool: Arc<Pool>,
}

struct Pool {
    url: String,
    idle: Mutex<Vec<Client>>,
    /// One permit per connection that may be in use; a transaction waits
    /// here when the pool is fully in use.
    permits: Arc<Semaphore>,
}

impl PostgresStore {
    /// Connects to the database at `url` (`postgres://user:pass@host/db`)
    /// with at most `max_connections` connections.
    pub async fn connect_async(url: &str, max_connections: usize) -> StoreResult<Self> {
        let first = open(url).await?;
        // One more connection, listening for changes other programs make.
        runtime::io().spawn(listen(url.to_string()));
        Ok(Self {
            pool: Arc::new(Pool {
                url: url.to_string(),
                idle: Mutex::new(vec![first]),
                permits: Arc::new(Semaphore::new(max_connections.max(1))),
            }),
        })
    }

    /// Blocking form of [`connect_async`](Self::connect_async).
    pub fn connect(url: &str, max_connections: usize) -> StoreResult<Self> {
        runtime::block_on(Self::connect_async(url, max_connections))
    }
}

/// Listens on `ortus_changes` and wakes this program's waiters on each
/// topic (see [`crate::changes`]); connects again after a loss.
async fn listen(url: String) {
    loop {
        if let Err(e) = listen_once(&url).await {
            eprintln!("ortus: listening for database changes: {e}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn listen_once(url: &str) -> StoreResult<()> {
    let settings = tls::Settings::from_url(url).map_err(StoreError::Database)?;
    let config: tokio_postgres::Config = settings.url.parse().map_err(error)?;
    if settings.mode == tls::Mode::Disable {
        let (client, connection) = config.connect(NoTls).await.map_err(error)?;
        let pump = runtime::io().spawn(pump(connection));
        client
            .batch_execute("LISTEN ortus_changes")
            .await
            .map_err(error)?;
        let _ = pump.await;
    } else {
        let mut config = config;
        config.ssl_mode(if settings.mode == tls::Mode::Prefer {
            tokio_postgres::config::SslMode::Prefer
        } else {
            tokio_postgres::config::SslMode::Require
        });
        let connector = tokio_postgres_rustls::MakeRustlsConnect::new(
            settings.client_config().map_err(StoreError::Database)?,
        );
        let (client, connection) = config.connect(connector).await.map_err(error)?;
        let pump = runtime::io().spawn(pump(connection));
        client
            .batch_execute("LISTEN ortus_changes")
            .await
            .map_err(error)?;
        let _ = pump.await;
    }
    Ok(())
}

/// Drives a listening connection and hands each notice to its waiters.
async fn pump<S, T>(mut connection: tokio_postgres::Connection<S, T>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let message = std::future::poll_fn(|cx| connection.poll_message(cx)).await;
        match message {
            Some(Ok(tokio_postgres::AsyncMessage::Notification(n))) => {
                crate::changes::wake(n.payload())
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return,
        }
    }
}

async fn open(url: &str) -> StoreResult<Client> {
    let settings = tls::Settings::from_url(url).map_err(StoreError::Database)?;
    let config: tokio_postgres::Config = settings.url.parse().map_err(error)?;
    let client = if settings.mode == tls::Mode::Disable {
        let (client, connection) = config.connect(NoTls).await.map_err(error)?;
        // The connection does its work on the process-wide runtime, so it
        // stays usable whichever runtime the caller is on.
        runtime::io().spawn(connection);
        client
    } else {
        let mut config = config;
        config.ssl_mode(if settings.mode == tls::Mode::Prefer {
            tokio_postgres::config::SslMode::Prefer
        } else {
            tokio_postgres::config::SslMode::Require
        });
        let connector = tokio_postgres_rustls::MakeRustlsConnect::new(
            settings.client_config().map_err(StoreError::Database)?,
        );
        let (client, connection) = config.connect(connector).await.map_err(error)?;
        runtime::io().spawn(connection);
        client
    };
    Ok(client)
}

/// TLS to PostgreSQL, set in the URL as libpq does:
///
/// - `sslmode=disable`: never.
/// - `sslmode=prefer` (the default): when the server offers it, without
///   checking its certificate (it protects against eavesdropping, not
///   against someone in the middle).
/// - `sslmode=require`: always; the certificate is checked only when
///   `sslrootcert` names a CA file.
/// - `sslmode=verify-ca` and `verify-full`: always, and the certificate
///   must chain to `sslrootcert` (or a public authority) and name the host.
pub(crate) mod tls {
    use std::sync::Arc;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mode {
        Disable,
        Prefer,
        Require,
        Verify,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub struct Settings {
        /// The URL without the options handled here.
        pub url: String,
        pub mode: Mode,
        pub root_cert: Option<String>,
    }

    impl Settings {
        pub fn from_url(url: &str) -> Result<Self, String> {
            let (base, query) = url.split_once('?').unwrap_or((url, ""));
            let mut mode = Mode::Prefer;
            let mut root_cert = None;
            let mut rest = Vec::new();
            for pair in query.split('&').filter(|p| !p.is_empty()) {
                match pair.split_once('=') {
                    Some(("sslmode", value)) => {
                        mode = match value {
                            "disable" => Mode::Disable,
                            "allow" | "prefer" => Mode::Prefer,
                            "require" => Mode::Require,
                            "verify-ca" | "verify-full" => Mode::Verify,
                            other => return Err(format!("sslmode '{other}' is not one of disable, prefer, require, verify-ca, verify-full")),
                        }
                    }
                    Some(("sslrootcert", value)) => root_cert = Some(value.replace("%2F", "/").replace("%2f", "/")),
                    _ => rest.push(pair),
                }
            }
            let url = if rest.is_empty() {
                base.to_string()
            } else {
                format!("{base}?{}", rest.join("&"))
            };
            Ok(Self {
                url,
                mode,
                root_cert,
            })
        }

        pub fn client_config(&self) -> Result<rustls::ClientConfig, String> {
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?;
            let verify = self.mode == Mode::Verify
                || (self.mode == Mode::Require && self.root_cert.is_some());
            if !verify {
                return Ok(builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(EncryptOnly(provider)))
                    .with_no_client_auth());
            }
            let mut roots = rustls::RootCertStore::empty();
            match &self.root_cert {
                Some(path) => {
                    let pem =
                        std::fs::read(path).map_err(|e| format!("sslrootcert {path}: {e}"))?;
                    for cert in rustls::pki_types::pem::PemObject::pem_slice_iter(&pem) {
                        let cert: CertificateDer<'static> =
                            cert.map_err(|e| format!("sslrootcert {path}: {e}"))?;
                        roots
                            .add(cert)
                            .map_err(|e| format!("sslrootcert {path}: {e}"))?;
                    }
                }
                None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
            }
            Ok(builder.with_root_certificates(roots).with_no_client_auth())
        }
    }

    /// Accepts the server's certificate without checking who issued it,
    /// but still checks the handshake signatures: the connection is
    /// encrypted, as libpq's `require` without a root certificate.
    #[derive(Debug)]
    struct EncryptOnly(Arc<rustls::crypto::CryptoProvider>);

    impl ServerCertVerifier for EncryptOnly {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ssl_options_are_read_from_the_url() {
            let s = Settings::from_url("postgres://u:p@db/ortus").unwrap();
            assert_eq!(
                (Mode::Prefer, None, "postgres://u:p@db/ortus"),
                (s.mode, s.root_cert.clone(), s.url.as_str())
            );
            let s = Settings::from_url("postgres://u:p@db/ortus?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem&options=-c%20x").unwrap();
            assert_eq!(Mode::Verify, s.mode);
            assert_eq!(Some("/etc/ca.pem".to_string()), s.root_cert);
            assert_eq!("postgres://u:p@db/ortus?options=-c%20x", s.url);
            assert_eq!(
                Mode::Disable,
                Settings::from_url("postgres://db/x?sslmode=disable")
                    .unwrap()
                    .mode
            );
            assert!(Settings::from_url("postgres://db/x?sslmode=sometimes").is_err());
            assert!(Settings::from_url("postgres://db/x?sslmode=require")
                .unwrap()
                .client_config()
                .is_ok());
            assert!(Settings::from_url("postgres://db/x?sslmode=verify-full")
                .unwrap()
                .client_config()
                .is_ok());
            assert!(Settings::from_url(
                "postgres://db/x?sslmode=verify-full&sslrootcert=/nonexistent"
            )
            .unwrap()
            .client_config()
            .is_err());
        }
    }
}

impl AsyncStore for PostgresStore {
    fn engine(&self) -> Engine {
        Engine::Postgres
    }

    fn begin(&self) -> BoxFuture<'_, StoreResult<Box<dyn AsyncTransaction>>> {
        Box::pin(self.start_transaction("BEGIN"))
    }

    fn begin_read(&self) -> BoxFuture<'_, StoreResult<Box<dyn AsyncTransaction>>> {
        Box::pin(self.start_transaction("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY"))
    }
}

impl PostgresStore {
    async fn start_transaction(&self, begin: &str) -> StoreResult<Box<dyn AsyncTransaction>> {
        let permit = tokio::time::timeout(
            super::BUSY_LIMIT,
            Arc::clone(&self.pool.permits).acquire_owned(),
        )
        .await
        .map_err(|_| StoreError::Database(super::BUSY_MESSAGE.into()))?
        .map_err(|_| StoreError::Database("the connection pool is closed".into()))?;
        let idle = self
            .pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop();
        let client = match idle {
            Some(client) if !client.is_closed() => client,
            _ => open(&self.pool.url).await?,
        };
        client.batch_execute(begin).await.map_err(error)?;
        Ok(Box::new(PostgresTransaction {
            client: Some(client),
            pool: Arc::clone(&self.pool),
            _permit: permit,
            failed: false,
        }) as Box<dyn AsyncTransaction>)
    }
}

struct PostgresTransaction {
    /// `None` once the transaction has ended. A transaction dropped while
    /// this is still `Some` closes its connection, and the server rolls
    /// the work back.
    client: Option<Client>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
    /// A statement failed: PostgreSQL has aborted the transaction, and a
    /// COMMIT would quietly roll back, so commit reports the failure.
    failed: bool,
}

impl PostgresTransaction {
    fn client(&self) -> StoreResult<&Client> {
        self.client
            .as_ref()
            .ok_or_else(|| StoreError::Database("the transaction has ended".into()))
    }

    async fn end(mut self: Box<Self>, statement: &str) -> StoreResult<()> {
        let Some(client) = self.client.take() else {
            return Ok(());
        };
        client.batch_execute(statement).await.map_err(error)?;
        self.pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(client);
        Ok(())
    }
}

impl AsyncTx for PostgresTransaction {
    fn execute<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<u64>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Postgres);
            let result = self
                .client()?
                .execute(sql.as_str(), &refs(params))
                .await
                .map_err(error);
            self.failed |= result.is_err();
            result
        })
    }

    fn query<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            let sql = dialect(sql, Engine::Postgres);
            let result = self
                .client()?
                .query(sql.as_str(), &refs(params))
                .await
                .map_err(error);
            self.failed |= result.is_err();
            let rows = result?;
            rows.iter().map(read_row).collect()
        })
    }
}

impl AsyncTransaction for PostgresTransaction {
    fn commit(self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(async move {
            if self.failed {
                self.end("ROLLBACK").await?;
                return Err(StoreError::Database(
                    "a statement in the transaction failed, so nothing was committed".into(),
                ));
            }
            self.end("COMMIT").await
        })
    }

    fn rollback(self: Box<Self>) -> BoxFuture<'static, StoreResult<()>> {
        Box::pin(self.end("ROLLBACK"))
    }
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect()
}

fn read_row(row: &tokio_postgres::Row) -> StoreResult<Row> {
    let mut values = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        let ty = column.type_();
        let value = if *ty == Type::VOID {
            Ok(Value::Null)
        } else if *ty == Type::INT8 {
            row.try_get::<_, Option<i64>>(index).map(Value::from)
        } else if *ty == Type::INT4 {
            row.try_get::<_, Option<i32>>(index)
                .map(|v| Value::from(v.map(i64::from)))
        } else if *ty == Type::INT2 {
            row.try_get::<_, Option<i16>>(index)
                .map(|v| Value::from(v.map(i64::from)))
        } else if *ty == Type::BOOL {
            row.try_get::<_, Option<bool>>(index)
                .map(|v| Value::from(v.map(i64::from)))
        } else if *ty == Type::BYTEA {
            row.try_get::<_, Option<Vec<u8>>>(index).map(Value::from)
        } else {
            row.try_get::<_, Option<String>>(index).map(Value::from)
        };
        values.push(value.map_err(error)?);
    }
    Ok(Row(values))
}

impl ToSql for Value {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut tokio_postgres::types::private::BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        match self {
            Value::Null => Ok(IsNull::Yes),
            Value::Int(v) if *ty == Type::INT4 => i32::try_from(*v)?.to_sql(ty, out),
            Value::Int(v) if *ty == Type::INT2 => i16::try_from(*v)?.to_sql(ty, out),
            Value::Int(v) if *ty == Type::BOOL => (*v != 0).to_sql(ty, out),
            Value::Int(v) => v.to_sql(ty, out),
            Value::Text(v) => v.to_sql(ty, out),
            Value::Blob(v) => v.to_sql(ty, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

fn error(e: tokio_postgres::Error) -> StoreError {
    // The server's own message says what went wrong; "db error" alone
    // does not.
    let message = match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    };
    match e.code() {
        Some(code) if *code == SqlState::UNIQUE_VIOLATION => StoreError::Conflict(message),
        _ => StoreError::Database(message),
    }
}
