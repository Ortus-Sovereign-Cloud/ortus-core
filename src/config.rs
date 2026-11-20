//! Configuration: what this program is and where its data lives.
//!
//! Settings come from a TOML file and from environment variables; a
//! variable overrides the file. With neither, the program runs every role
//! on SQLite and a local directory under `./data`.
//!
//! ```toml
//! roles = ["all"]              # all, web, mail, media, worker
//! data_dir = "/var/lib/ortus"
//!
//! [web]
//! ip = "0.0.0.0"
//! port = 7878
//! public_url = "https://ortus.example.org"   # where people reach it
//!
//! [database]                   # omit for SQLite in data_dir
//! url = "postgres://ortus:secret@db/ortus"
//! max_connections = 16
//!
//! [apps]
//! off = ["chat"]               # apps that start switched off
//!
//! [outgoing_mail]              # mail from Ortus itself (codes, notices)
//! host = "smtp.example.org"
//! port = 587
//! security = "starttls"        # starttls, tls or none
//! username = "ortus"
//! password = "..."
//! from = "Ortus <noreply@example.org>"
//!
//! [signup]                     # open sign-up, for a public demo
//! open = false
//! accounts_per_day = 200
//! accounts_per_address_per_day = 3
//! account_days = 30            # then the account and its data are deleted
//! quota_mb = 100
//!
//! [site]
//! website = false              # the project website at the root (demo)
//!
//! [files.s3]                   # omit for a directory in data_dir
//! endpoint = "http://rustfs:9000"
//! bucket = "ortus"
//! access_key = "..."
//! secret_key = "..."
//! ```
//!
//! | Variable | Setting |
//! | --- | --- |
//! | `ORTUS_CONFIG` | path of the TOML file (default `ortus.toml`, if present) |
//! | `ORTUS_ROLES` | `roles`, comma separated |
//! | `ORTUS_DATA_DIR` | `data_dir` |
//! | `ORTUS_WEB_IP`, `ORTUS_WEB_PORT`, `ORTUS_PUBLIC_URL` | `[web]` |
//! | `ORTUS_DATABASE_URL`, `ORTUS_DATABASE_MAX_CONNECTIONS` | `[database]` |
//! | `ORTUS_APPS_OFF` | `[apps] off`, comma separated |
//! | `ORTUS_SETUP_TOKEN` | the first-start token (environment only) |
//! | `ORTUS_MAIL_HOST`, `_PORT`, `_SECURITY`, `_USERNAME`, `_PASSWORD`, `_FROM` | `[outgoing_mail]` |
//! | `ORTUS_SIGNUP_OPEN`, `_ACCOUNTS_PER_DAY`, `_ACCOUNTS_PER_ADDRESS_PER_DAY`, `_ACCOUNT_DAYS`, `_QUOTA_MB` | `[signup]` |
//! | `ORTUS_SITE_WEBSITE` | `[site] website` |
//! | `ORTUS_FILES_S3_ENDPOINT`, `_BUCKET`, `_REGION`, `_ACCESS_KEY`, `_SECRET_KEY`, `_ROOT` | `[files.s3]` |

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::files::{Files, FilesError, S3Settings};
use crate::storage::{AsyncStore, PostgresStore, SqliteStore, StoreError};

/// One part of the work an Ortus program can do. The single-machine
/// program runs all of them; the Kubernetes chart runs one workload each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Serves the web client and the public API.
    Web,
    /// Receives and sends mail.
    Mail,
    /// Relays call media.
    Media,
    /// Runs background jobs.
    Worker,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Web, Role::Mail, Role::Media, Role::Worker];

    pub fn name(self) -> &'static str {
        match self {
            Role::Web => "web",
            Role::Mail => "mail",
            Role::Media => "media",
            Role::Worker => "worker",
        }
    }
}

/// A network listener a program opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub role: Role,
    pub protocol: &'static str,
    pub ip: String,
    pub port: u16,
}

/// Where the database is.
#[derive(Clone, PartialEq, Eq)]
pub enum Database {
    Sqlite { file: PathBuf },
    Postgres { url: String, max_connections: usize },
}

/// Where file contents are kept.
#[derive(Clone, Debug)]
pub enum FileStore {
    Local { dir: PathBuf },
    S3(S3Settings),
}

/// The settings of one program. Its `Debug` output leaves secrets out.
#[derive(Clone, Debug)]
pub struct Config {
    pub roles: BTreeSet<Role>,
    pub data_dir: PathBuf,
    pub web_ip: String,
    pub web_port: u16,
    /// The address people reach this installation at, without a trailing
    /// slash: in links, cookies, passkeys and sign-in with other services.
    pub public_url: String,
    pub database: Database,
    pub files: FileStore,
    /// Apps that are off until an administrator turns them on.
    pub apps_off: Vec<String>,
    /// Where mail from Ortus itself (codes, notices) is handed over, if
    /// anywhere.
    pub outgoing_mail: Option<OutgoingMail>,
    pub signup: Signup,
    /// Whether the root address shows the project website to visitors
    /// (the public demo only).
    pub website: bool,
    /// The first-start token, when an automated install chooses it
    /// (`ORTUS_SETUP_TOKEN`, environment only); otherwise a random one.
    pub setup_token: Option<Secret>,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Database::Sqlite { file } => f.debug_struct("Sqlite").field("file", file).finish(),
            Database::Postgres {
                url,
                max_connections,
            } => f
                .debug_struct("Postgres")
                .field("url", &redact_url(url))
                .field("max_connections", max_connections)
                .finish(),
        }
    }
}

/// How mail is handed to the mail server: SMTP submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingMail {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: Option<String>,
    pub password: Option<Secret>,
    /// The sender, as `Ortus <noreply@example.org>`.
    pub from: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailSecurity {
    /// Plain connection upgraded with STARTTLS (port 587).
    StartTls,
    /// TLS from the first byte (port 465).
    Tls,
    /// No encryption: only for a server on the same machine.
    None,
}

/// Open sign-up, for a public demo: anyone may make an account after an
/// email check, within these limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signup {
    pub open: bool,
    pub accounts_per_day: u32,
    pub accounts_per_address_per_day: u32,
    /// Accounts made by sign-up are deleted with their data after this.
    pub account_days: u32,
    pub quota_mb: u64,
}

/// A secret setting, shown as `***` in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl Secret {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A database URL with its password replaced by `***`.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some((credentials, host)) = rest.rsplit_once('@') else {
        return url.to_string();
    };
    match credentials.split_once(':') {
        Some((user, _)) => format!("{scheme}://{user}:***@{host}"),
        None => url.to_string(),
    }
}

/// A configuration that cannot be used, with the setting at fault.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "configuration: {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    roles: Option<Vec<String>>,
    data_dir: Option<String>,
    #[serde(default)]
    web: WebSection,
    #[serde(default)]
    database: DatabaseSection,
    #[serde(default)]
    files: FilesSection,
    #[serde(default)]
    apps: AppsSection,
    #[serde(default)]
    outgoing_mail: MailSection,
    #[serde(default)]
    signup: SignupSection,
    #[serde(default)]
    site: SiteSection,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSection {
    host: Option<String>,
    port: Option<u16>,
    security: Option<String>,
    username: Option<String>,
    password: Option<String>,
    from: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignupSection {
    open: Option<bool>,
    accounts_per_day: Option<u32>,
    accounts_per_address_per_day: Option<u32>,
    account_days: Option<u32>,
    quota_mb: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteSection {
    website: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppsSection {
    off: Option<Vec<String>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebSection {
    ip: Option<String>,
    port: Option<u16>,
    public_url: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseSection {
    url: Option<String>,
    max_connections: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesSection {
    #[serde(default)]
    s3: S3Section,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct S3Section {
    endpoint: Option<String>,
    bucket: Option<String>,
    region: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    root: Option<String>,
}

impl Config {
    /// Whether people reach this installation over HTTPS, so cookies must
    /// say Secure.
    pub fn secure(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// Reads the configuration of this process: the file named by
    /// `ORTUS_CONFIG` (or `ortus.toml` when it exists), then the
    /// environment.
    pub fn from_process() -> Result<Self, ConfigError> {
        let env = |name: &str| std::env::var(name).ok();
        let file = match env("ORTUS_CONFIG") {
            Some(path) => Some(
                std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError(format!("cannot read {path}: {e}")))?,
            ),
            None => std::fs::read_to_string("ortus.toml").ok(),
        };
        Self::load(file.as_deref(), &env)
    }

    /// Builds the configuration from the text of a TOML file, if any, and
    /// a way to look up environment variables.
    pub fn load(
        file: Option<&str>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let file: FileConfig = match file {
            Some(text) => toml::from_str(text).map_err(|e| {
                ConfigError(format!(
                    "the configuration file is not valid: {}",
                    e.message()
                ))
            })?,
            None => FileConfig::default(),
        };
        let env = |name: &str| env(name).filter(|value| !value.trim().is_empty());

        let role_names = match env("ORTUS_ROLES") {
            Some(list) => list
                .split(',')
                .map(|name| name.trim().to_string())
                .collect(),
            None => file.roles.unwrap_or_else(|| vec!["all".into()]),
        };
        let mut roles = BTreeSet::new();
        for name in &role_names {
            match name.as_str() {
                "all" => roles.extend(Role::ALL),
                other => match Role::ALL.iter().find(|role| role.name() == other) {
                    Some(role) => drop(roles.insert(*role)),
                    None => {
                        return Err(ConfigError(format!(
                            "unknown role '{other}'; roles are all, web, mail, media, worker"
                        )))
                    }
                },
            }
        }
        if roles.is_empty() {
            return Err(ConfigError(
                "no roles are set; use all, web, mail, media or worker".into(),
            ));
        }

        let data_dir = PathBuf::from(
            env("ORTUS_DATA_DIR")
                .or(file.data_dir)
                .unwrap_or_else(|| "data".into()),
        );
        let web_ip = env("ORTUS_WEB_IP")
            .or(file.web.ip)
            .unwrap_or_else(|| "127.0.0.1".into());
        let web_port = match env("ORTUS_WEB_PORT") {
            Some(text) => text.parse().map_err(|_| {
                ConfigError(format!("ORTUS_WEB_PORT is '{text}', not a port number"))
            })?,
            None => file.web.port.unwrap_or(7878),
        };

        let max_connections = match env("ORTUS_DATABASE_MAX_CONNECTIONS") {
            Some(text) => text.parse().map_err(|_| {
                ConfigError(format!(
                    "ORTUS_DATABASE_MAX_CONNECTIONS is '{text}', not a number"
                ))
            })?,
            None => file.database.max_connections.unwrap_or(16),
        };
        let database = match env("ORTUS_DATABASE_URL").or(file.database.url) {
            Some(url) if url.starts_with("postgres://") || url.starts_with("postgresql://") => {
                Database::Postgres {
                    url,
                    max_connections,
                }
            }
            Some(url) => {
                return Err(ConfigError(format!(
                    "the database URL must start with postgres://, got '{}'",
                    url.split(':').next().unwrap_or_default()
                )))
            }
            None => Database::Sqlite {
                file: data_dir.join("ortus.db"),
            },
        };

        let s3 = file.files.s3;
        let setting = |variable: &str, from_file: Option<String>| env(variable).or(from_file);
        let endpoint = setting("ORTUS_FILES_S3_ENDPOINT", s3.endpoint);
        let bucket = setting("ORTUS_FILES_S3_BUCKET", s3.bucket);
        let access_key = setting("ORTUS_FILES_S3_ACCESS_KEY", s3.access_key);
        let secret_key = setting("ORTUS_FILES_S3_SECRET_KEY", s3.secret_key);
        let region = setting("ORTUS_FILES_S3_REGION", s3.region);
        let root = setting("ORTUS_FILES_S3_ROOT", s3.root);
        let files = match (endpoint, bucket, access_key, secret_key) {
            (None, None, None, None) => FileStore::Local { dir: data_dir.join("files") },
            (Some(endpoint), Some(bucket), Some(access_key), Some(secret_key)) => FileStore::S3(S3Settings { endpoint, bucket, region: region.unwrap_or_else(|| "us-east-1".into()), access_key, secret_key, root: root.unwrap_or_default() }),
            _ => return Err(ConfigError("S3 file storage needs endpoint, bucket, access_key and secret_key; some are missing".into())),
        };

        let public_url = env("ORTUS_PUBLIC_URL")
            .or(file.web.public_url)
            .unwrap_or_else(|| format!("http://{web_ip}:{web_port}"))
            .trim_end_matches('/')
            .to_string();
        if !(public_url.starts_with("https://") || public_url.starts_with("http://")) {
            return Err(ConfigError(format!(
                "public_url '{public_url}' must start with https:// or http://"
            )));
        }
        let number = |name: &str, value: Option<String>| -> Result<Option<u64>, ConfigError> {
            value
                .map(|text| {
                    text.parse::<u64>()
                        .map_err(|_| ConfigError(format!("{name} is '{text}', not a number")))
                })
                .transpose()
        };
        let flag = |name: &str, value: Option<String>| -> Result<Option<bool>, ConfigError> {
            value
                .map(|text| match text.as_str() {
                    "1" | "true" | "yes" | "on" => Ok(true),
                    "0" | "false" | "no" | "off" => Ok(false),
                    other => Err(ConfigError(format!(
                        "{name} is '{other}', not true or false"
                    ))),
                })
                .transpose()
        };
        let mail = file.outgoing_mail;
        let outgoing_mail = match env("ORTUS_MAIL_HOST").or(mail.host) {
            None => None,
            Some(host) => {
                let security = match env("ORTUS_MAIL_SECURITY")
                    .or(mail.security)
                    .as_deref()
                    .unwrap_or("starttls")
                {
                    "starttls" => MailSecurity::StartTls,
                    "tls" => MailSecurity::Tls,
                    "none" => MailSecurity::None,
                    other => {
                        return Err(ConfigError(format!(
                            "outgoing_mail.security is '{other}'; use starttls, tls or none"
                        )))
                    }
                };
                let default_port = match security {
                    MailSecurity::Tls => 465,
                    MailSecurity::StartTls => 587,
                    MailSecurity::None => 25,
                };
                let port = number("ORTUS_MAIL_PORT", env("ORTUS_MAIL_PORT"))?
                    .map(|p| p as u16)
                    .or(mail.port)
                    .unwrap_or(default_port);
                let from = env("ORTUS_MAIL_FROM").or(mail.from).ok_or_else(|| {
                    ConfigError("outgoing_mail.from is needed with outgoing_mail.host".into())
                })?;
                Some(OutgoingMail {
                    host,
                    port,
                    security,
                    username: env("ORTUS_MAIL_USERNAME").or(mail.username),
                    password: env("ORTUS_MAIL_PASSWORD").or(mail.password).map(Secret),
                    from,
                })
            }
        };
        let signup = Signup {
            open: flag("ORTUS_SIGNUP_OPEN", env("ORTUS_SIGNUP_OPEN"))?
                .or(file.signup.open)
                .unwrap_or(false),
            accounts_per_day: number(
                "ORTUS_SIGNUP_ACCOUNTS_PER_DAY",
                env("ORTUS_SIGNUP_ACCOUNTS_PER_DAY"),
            )?
            .map(|n| n as u32)
            .or(file.signup.accounts_per_day)
            .unwrap_or(200),
            accounts_per_address_per_day: number(
                "ORTUS_SIGNUP_ACCOUNTS_PER_ADDRESS_PER_DAY",
                env("ORTUS_SIGNUP_ACCOUNTS_PER_ADDRESS_PER_DAY"),
            )?
            .map(|n| n as u32)
            .or(file.signup.accounts_per_address_per_day)
            .unwrap_or(3),
            account_days: number(
                "ORTUS_SIGNUP_ACCOUNT_DAYS",
                env("ORTUS_SIGNUP_ACCOUNT_DAYS"),
            )?
            .map(|n| n as u32)
            .or(file.signup.account_days)
            .unwrap_or(30),
            quota_mb: number("ORTUS_SIGNUP_QUOTA_MB", env("ORTUS_SIGNUP_QUOTA_MB"))?
                .or(file.signup.quota_mb)
                .unwrap_or(100),
        };
        if signup.open && outgoing_mail.is_none() {
            return Err(ConfigError(
                "signup.open needs outgoing_mail, to send the email check".into(),
            ));
        }
        let website = flag("ORTUS_SITE_WEBSITE", env("ORTUS_SITE_WEBSITE"))?
            .or(file.site.website)
            .unwrap_or(false);
        let apps_off = match env("ORTUS_APPS_OFF") {
            Some(list) => list
                .split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect(),
            None => file.apps.off.unwrap_or_default(),
        };
        Ok(Self {
            roles,
            data_dir,
            web_ip,
            web_port,
            public_url,
            database,
            files,
            apps_off,
            outgoing_mail,
            signup,
            website,
            setup_token: env("ORTUS_SETUP_TOKEN").map(Secret),
        })
    }

    pub fn has(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }

    /// The listeners this program opens: those of its roles and no others.
    ///
    /// The mail and media roles have no listeners yet; theirs are added
    /// with the mail and calls slices.
    pub fn listeners(&self) -> Vec<Listener> {
        let mut listeners = Vec::new();
        if self.has(Role::Web) {
            listeners.push(Listener {
                role: Role::Web,
                protocol: "http",
                ip: self.web_ip.clone(),
                port: self.web_port,
            });
        }
        listeners
    }

    /// Opens the configured database.
    pub async fn open_store(&self) -> Result<Arc<dyn AsyncStore>, StoreError> {
        match &self.database {
            Database::Sqlite { file } => {
                if let Some(dir) = file.parent() {
                    create_dir(dir).map_err(StoreError::Database)?;
                }
                Ok(Arc::new(SqliteStore::open(file)?))
            }
            Database::Postgres {
                url,
                max_connections,
            } => Ok(Arc::new(
                PostgresStore::connect_async(url, *max_connections).await?,
            )),
        }
    }

    /// Opens the configured file store.
    pub fn open_files(&self) -> Result<Files, FilesError> {
        match &self.files {
            FileStore::Local { dir } => {
                create_dir(dir).map_err(FilesError::Store)?;
                Files::local(dir)
            }
            FileStore::S3(settings) => Files::s3(settings),
        }
    }
}

fn create_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(file: Option<&str>, vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::load(file, &|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        })
    }

    #[test]
    fn with_no_settings_one_program_does_everything_on_local_storage() {
        let config = load(None, &[]).unwrap();
        assert_eq!(
            Role::ALL.to_vec(),
            config.roles.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(
            Database::Sqlite {
                file: "data/ortus.db".into()
            },
            config.database
        );
        assert!(
            matches!(&config.files, FileStore::Local { dir } if dir == Path::new("data/files"))
        );
        assert_eq!(
            vec![Listener {
                role: Role::Web,
                protocol: "http",
                ip: "127.0.0.1".into(),
                port: 7878
            }],
            config.listeners()
        );
    }

    #[test]
    fn a_file_sets_everything_and_the_environment_overrides_it() {
        let file = r#"
            roles = ["web", "worker"]
            data_dir = "/var/lib/ortus"
            [web]
            ip = "0.0.0.0"
            port = 8080
            [database]
            url = "postgres://ortus:secret@db/ortus"
            max_connections = 4
            [files.s3]
            endpoint = "http://rustfs:9000"
            bucket = "ortus"
            access_key = "key"
            secret_key = "secret"
            [apps]
            off = ["chat", "calls"]
        "#;
        let config = load(Some(file), &[]).unwrap();
        assert_eq!(vec!["chat", "calls"], config.apps_off);
        assert_eq!(
            vec![Role::Web, Role::Worker],
            config.roles.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(
            Database::Postgres {
                url: "postgres://ortus:secret@db/ortus".into(),
                max_connections: 4
            },
            config.database
        );
        assert!(
            matches!(&config.files, FileStore::S3(s3) if s3.bucket == "ortus" && s3.region == "us-east-1" && s3.root.is_empty())
        );
        assert_eq!(("0.0.0.0", 8080), (config.web_ip.as_str(), config.web_port));

        let config = load(
            Some(file),
            &[
                ("ORTUS_ROLES", "mail"),
                ("ORTUS_WEB_PORT", "9090"),
                ("ORTUS_FILES_S3_BUCKET", "other"),
                ("ORTUS_DATABASE_MAX_CONNECTIONS", "2"),
                ("ORTUS_APPS_OFF", " drive ,,"),
            ],
        )
        .unwrap();
        assert_eq!(vec!["drive"], config.apps_off);
        assert_eq!(
            vec![Role::Mail],
            config.roles.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(9090, config.web_port);
        assert!(matches!(&config.files, FileStore::S3(s3) if s3.bucket == "other"));
        assert!(matches!(
            config.database,
            Database::Postgres {
                max_connections: 2,
                ..
            }
        ));
    }

    #[test]
    fn each_role_opens_only_its_own_listeners() {
        for (roles, expected) in [
            ("web", vec![Role::Web]),
            ("worker", vec![]),
            ("mail", vec![]),
            ("media", vec![]),
            ("worker,mail,media", vec![]),
            ("all", vec![Role::Web]),
            ("worker, web", vec![Role::Web]),
        ] {
            let config = load(None, &[("ORTUS_ROLES", roles)]).unwrap();
            assert_eq!(
                expected,
                config
                    .listeners()
                    .iter()
                    .map(|listener| listener.role)
                    .collect::<Vec<_>>(),
                "roles = {roles}"
            );
        }
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let config = load(
            None,
            &[
                ("ORTUS_DATABASE_URL", "postgres://ortus:hunter2@db/ortus"),
                ("ORTUS_FILES_S3_ENDPOINT", "http://s3"),
                ("ORTUS_FILES_S3_BUCKET", "b"),
                ("ORTUS_FILES_S3_ACCESS_KEY", "key"),
                ("ORTUS_FILES_S3_SECRET_KEY", "s3cret-value"),
                ("ORTUS_SETUP_TOKEN", "setup-secret"),
            ],
        )
        .unwrap();
        let shown = format!("{config:?}");
        for secret in ["hunter2", "s3cret-value", "setup-secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("postgres://ortus:***@db/ortus"), "{shown}");
    }

    #[test]
    fn mistakes_are_named() {
        let error = |file: Option<&str>, vars: &[(&str, &str)]| load(file, vars).unwrap_err().0;
        assert!(error(None, &[("ORTUS_ROLES", "web,mial")]).contains("unknown role 'mial'"));
        assert!(error(Some("roles = []"), &[]).contains("no roles"));
        assert!(error(None, &[("ORTUS_WEB_PORT", "http")]).contains("not a port number"));
        assert!(error(None, &[("ORTUS_DATABASE_URL", "mysql://db/ortus")])
            .contains("must start with postgres://"));
        assert!(error(None, &[("ORTUS_FILES_S3_BUCKET", "ortus")]).contains("some are missing"));
        assert!(error(Some("rolse = [\"web\"]"), &[]).contains("not valid"));
        assert!(error(Some("[web]\nport = 70000"), &[]).contains("not valid"));
        // A secret in the URL must not be echoed back in the error.
        assert!(!error(
            None,
            &[("ORTUS_DATABASE_URL", "mysql://user:hunter2@db/ortus")]
        )
        .contains("hunter2"));
    }
}
