//! The PostgreSQL engine, used by the Kubernetes chart.

use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{to_sql_checked, IsNull, ToSql, Type};
use tokio_postgres::{Client, NoTls, Statement};

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
    idle: Mutex<Vec<Conn>>,
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
                idle: Mutex::new(vec![Conn::new(first)]),
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

/// Statements kept prepared on each connection, at most.
const PREPARED: usize = 512;

/// Bumped by every schema change this program makes, so connections drop
/// statements prepared against the old schema.
static SCHEMA_CHANGES: AtomicU64 = AtomicU64::new(0);

/// A connection, with the statements it has prepared: each is parsed and
/// planned by the server once, then only bound and run (one round trip
/// instead of two).
struct Conn {
    client: Client,
    prepared: HashMap<String, Statement>,
    schema: u64,
}

/// What a statement gives back.
enum Output {
    Rows(Vec<tokio_postgres::Row>),
    Count(u64),
}

/// Statements worth keeping prepared: ordinary reads and writes. Schema
/// changes, cursors and the rest are prepared for the one call, as before.
fn keeps(sql: &str) -> bool {
    let head = sql.trim_start().get(..6).unwrap_or_default();
    ["SELECT", "INSERT", "UPDATE", "DELETE", "WITH "]
        .iter()
        .any(|k| head.len() >= k.len() && head[..k.len()].eq_ignore_ascii_case(k))
}

fn changes_schema(sql: &str) -> bool {
    let head = sql.trim_start();
    ["CREATE", "ALTER", "DROP", "TRUNCATE"].iter().any(|k| {
        head.get(..k.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(k))
    })
}

impl Conn {
    fn new(client: Client) -> Self {
        Self {
            client,
            prepared: HashMap::new(),
            schema: SCHEMA_CHANGES.load(Ordering::Acquire),
        }
    }

    /// Runs one statement. `begin`, when given, is sent first in the same
    /// round trip (a transaction starts with its first statement).
    async fn run(
        &mut self,
        begin: Option<&str>,
        sql: &str,
        params: &[Value],
        rows: bool,
    ) -> Result<Output, tokio_postgres::Error> {
        let started = Instant::now();
        let result = self.run_timed(begin, sql, params, rows).await;
        crate::request::statement(started.elapsed());
        if let Err(e) = &result {
            // "cached plan must not change result type": a table changed
            // under a prepared statement (another program's upgrade).
            if e.code() == Some(&SqlState::FEATURE_NOT_SUPPORTED) {
                self.prepared.clear();
            }
        }
        result
    }

    async fn run_timed(
        &mut self,
        mut begin: Option<&str>,
        sql: &str,
        params: &[Value],
        rows: bool,
    ) -> Result<Output, tokio_postgres::Error> {
        let schema = SCHEMA_CHANGES.load(Ordering::Acquire);
        if schema != self.schema {
            self.prepared.clear();
            self.schema = schema;
        }
        let client = &self.client;
        let values = refs(params);
        if !keeps(sql) {
            if let Some(begin) = begin {
                client.batch_execute(begin).await?;
            }
            let output = if rows {
                Output::Rows(client.query(sql, &values).await?)
            } else {
                Output::Count(client.execute(sql, &values).await?)
            };
            if changes_schema(sql) {
                SCHEMA_CHANGES.fetch_add(1, Ordering::AcqRel);
                self.prepared.clear();
            }
            return Ok(output);
        }
        let statement = match self.prepared.get(sql) {
            Some(statement) => statement.clone(),
            None => {
                let statement = match begin.take() {
                    Some(begin) => {
                        let (began, prepared) =
                            tokio::join!(client.batch_execute(begin), client.prepare(sql));
                        began?;
                        prepared?
                    }
                    None => client.prepare(sql).await?,
                };
                if self.prepared.len() >= PREPARED {
                    self.prepared.clear();
                }
                self.prepared.insert(sql.to_string(), statement.clone());
                statement
            }
        };
        let run = async {
            if rows {
                client.query(&statement, &values).await.map(Output::Rows)
            } else {
                client.execute(&statement, &values).await.map(Output::Count)
            }
        };
        match begin {
            Some(begin) => {
                let (began, output) = tokio::join!(client.batch_execute(begin), run);
                began?;
                output
            }
            None => run.await,
        }
    }
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

    // One statement on its own needs no BEGIN and COMMIT around it: it is
    // a transaction by itself.
    fn execute<'a>(&'a self, sql: &'a str, params: &'a [Value]) -> BoxFuture<'a, StoreResult<u64>> {
        Box::pin(async move {
            match self.alone(sql, params, false).await? {
                Output::Count(n) => Ok(n),
                Output::Rows(rows) => Ok(rows.len() as u64),
            }
        })
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            match self.alone(sql, params, true).await? {
                Output::Rows(rows) => rows.iter().map(read_row).collect(),
                Output::Count(_) => Ok(Vec::new()),
            }
        })
    }
}

impl PostgresStore {
    /// A connection of the pool, and the permit to use it.
    async fn take(&self) -> StoreResult<(Conn, OwnedSemaphorePermit)> {
        let started = Instant::now();
        let permit = tokio::time::timeout(
            super::BUSY_LIMIT,
            Arc::clone(&self.pool.permits).acquire_owned(),
        )
        .await
        .map_err(|_| StoreError::Database(super::BUSY_MESSAGE.into()))?
        .map_err(|_| StoreError::Database("the connection pool is closed".into()))?;
        crate::request::waited(started.elapsed());
        let idle = self
            .pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop();
        let conn = match idle {
            Some(conn) if !conn.client.is_closed() => conn,
            _ => {
                crate::request::opened();
                Conn::new(open(&self.pool.url).await?)
            }
        };
        Ok((conn, permit))
    }

    fn give_back(&self, conn: Conn) {
        self.pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(conn);
    }

    /// One statement outside any transaction. The connection goes back to
    /// the pool after it, also after an error the server reported; one
    /// dropped half-way (the request went away) is closed instead.
    async fn alone(&self, sql: &str, params: &[Value], rows: bool) -> StoreResult<Output> {
        let (mut conn, _permit) = self.take().await?;
        let sql = dialect(sql, Engine::Postgres);
        let result = conn.run(None, &sql, params, rows).await;
        if result
            .as_ref()
            .map_or_else(|e| e.as_db_error().is_some(), |_| true)
        {
            self.give_back(conn);
        }
        result.map_err(error)
    }

    async fn start_transaction(
        &self,
        begin: &'static str,
    ) -> StoreResult<Box<dyn AsyncTransaction>> {
        let (conn, permit) = self.take().await?;
        // BEGIN goes with the first statement, saving a round trip.
        Ok(Box::new(PostgresTransaction {
            conn: Some(conn),
            begin: Some(begin),
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
    conn: Option<Conn>,
    /// The BEGIN not yet sent: no statement has run yet.
    begin: Option<&'static str>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
    /// A statement failed: PostgreSQL has aborted the transaction, and a
    /// COMMIT would quietly roll back, so commit reports the failure.
    failed: bool,
}

impl PostgresTransaction {
    async fn run(&mut self, sql: &str, params: &[Value], rows: bool) -> StoreResult<Output> {
        let sql = dialect(sql, Engine::Postgres);
        let begin = self.begin.take();
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| StoreError::Database("the transaction has ended".into()))?;
        let result = conn.run(begin, &sql, params, rows).await.map_err(error);
        self.failed |= result.is_err();
        result
    }

    async fn end(mut self: Box<Self>, statement: &str) -> StoreResult<()> {
        let Some(conn) = self.conn.take() else {
            return Ok(());
        };
        // Nothing ran, so nothing was begun.
        if self.begin.is_none() {
            conn.client.batch_execute(statement).await.map_err(error)?;
        }
        self.pool
            .idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(conn);
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
            match self.run(sql, params, false).await? {
                Output::Count(n) => Ok(n),
                Output::Rows(rows) => Ok(rows.len() as u64),
            }
        })
    }

    fn query<'a>(
        &'a mut self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, StoreResult<Vec<Row>>> {
        Box::pin(async move {
            match self.run(sql, params, true).await? {
                Output::Rows(rows) => rows.iter().map(read_row).collect(),
                Output::Count(_) => Ok(Vec::new()),
            }
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
