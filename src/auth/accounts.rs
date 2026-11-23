//! Accounts: the people who sign in, and their role in the installation.

use crate::storage::{AsyncStore, AsyncTx, Migration, Row, StoreError, StoreResult, Value};

pub(super) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "accounts, credentials and sessions",
        sql: "CREATE TABLE core_users (
        id TEXT PRIMARY KEY,
        email TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        role TEXT NOT NULL,
        state TEXT NOT NULL,
        language TEXT,
        last_app TEXT,
        chooser_seen BIGINT NOT NULL,
        expires_at BIGINT,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE core_credentials (
        id TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        data TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        last_used_at BIGINT
    );
    CREATE INDEX core_credentials_user ON core_credentials (user_id, kind);
    CREATE TABLE core_sessions (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        methods TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        last_seen_at BIGINT NOT NULL,
        expires_at BIGINT NOT NULL,
        address TEXT NOT NULL,
        agent TEXT NOT NULL
    );
    CREATE INDEX core_sessions_user ON core_sessions (user_id)",
    },
    Migration {
        version: 2,
        name: "sign-in tickets and limits",
        sql: "CREATE TABLE core_signin_tickets (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        methods TEXT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE TABLE core_signin_limits (
        key TEXT PRIMARY KEY,
        failures BIGINT NOT NULL,
        blocked_until BIGINT NOT NULL
    )",
    },
    Migration {
        version: 3,
        name: "passkey challenges",
        sql: "CREATE TABLE core_webauthn_challenges (
        challenge TEXT PRIMARY KEY,
        user_id TEXT,
        purpose TEXT NOT NULL,
        expires_at BIGINT NOT NULL
    )",
    },
    Migration {
        version: 4,
        name: "sign-up and quotas",
        sql: "CREATE TABLE core_email_checks (
        ticket_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        code_hash TEXT NOT NULL,
        attempts BIGINT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE TABLE core_signups (
        user_id TEXT PRIMARY KEY,
        address TEXT NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE INDEX core_signups_when ON core_signups (created_at);
    ALTER TABLE core_users ADD COLUMN quota_bytes BIGINT",
    },
    Migration {
        version: 5,
        name: "sign-up mails sent",
        sql: "CREATE TABLE core_signup_mails (
        email TEXT NOT NULL,
        address TEXT NOT NULL,
        sent_at BIGINT NOT NULL
    );
    CREATE INDEX core_signup_mails_email ON core_signup_mails (email, sent_at);
    CREATE INDEX core_signup_mails_address ON core_signup_mails (address, sent_at)",
    },
    Migration {
        version: 6,
        name: "sessions that confirmed it is them",
        sql: "ALTER TABLE core_sessions ADD COLUMN confirmed_at BIGINT",
    },
];

/// What a person may do in the installation as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Member,
    Admin,
    /// Can do everything, including changing how people sign in.
    Owner,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Member => "member",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "member" => Some(Role::Member),
            "admin" => Some(Role::Admin),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }
}

/// Whether an account can be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Created by an administrator; the person has not signed in yet.
    Invited,
    /// Signed up and waiting for the email check.
    Unverified,
    Active,
    Suspended,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Invited => "invited",
            State::Unverified => "unverified",
            State::Active => "active",
            State::Suspended => "suspended",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "invited" => Some(State::Invited),
            "unverified" => Some(State::Unverified),
            "active" => Some(State::Active),
            "suspended" => Some(State::Suspended),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: String,
    /// Lowercase, the address people sign in with.
    pub email: String,
    pub name: String,
    pub role: Role,
    pub state: State,
    /// The language chosen in personal settings, if any.
    pub language: Option<String>,
    /// The app opened last, to open again after sign-in.
    pub last_app: Option<String>,
    /// Whether the person has seen the app chooser after their first sign-in.
    pub chooser_seen: bool,
    /// When the account and its data are deleted (demo accounts only).
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

const COLUMNS: &str =
    "id, email, name, role, state, language, last_app, chooser_seen, expires_at, created_at";

fn from_row(row: &Row) -> StoreResult<User> {
    let text = |i: usize| row.text(i).unwrap_or_default().to_string();
    Ok(User {
        id: text(0),
        email: text(1),
        name: text(2),
        role: Role::parse(row.text(3).unwrap_or_default())
            .ok_or_else(|| StoreError::Database(format!("unknown role for user {}", text(0))))?,
        state: State::parse(row.text(4).unwrap_or_default())
            .ok_or_else(|| StoreError::Database(format!("unknown state for user {}", text(0))))?,
        language: row.text(5).map(str::to_string),
        last_app: row.text(6).map(str::to_string),
        chooser_seen: row.int(7) == Some(1),
        expires_at: row.int(8),
        created_at: row.int(9).unwrap_or_default(),
    })
}

/// An email address as stored: trimmed and lowercase. `None` when it is
/// not an address.
pub fn normal_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && email.len() <= 254
        && !email.chars().any(|c| {
            c.is_whitespace() || c.is_control() || c == '<' || c == '>' || c == ',' || c == ';'
        })
        && domain
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-');
    ok.then_some(email)
}

/// A new account.
pub struct NewUser<'a> {
    pub email: &'a str,
    pub name: &'a str,
    pub role: Role,
    pub state: State,
    pub expires_at: Option<i64>,
}

/// Creates an account in `tx`. Fails with a conflict when the address is
/// taken.
pub async fn create_in(tx: &mut dyn AsyncTx, user: &NewUser<'_>, now_ms: i64) -> StoreResult<User> {
    let email = normal_email(user.email)
        .ok_or_else(|| StoreError::Database(format!("'{}' is not an email address", user.email)))?;
    let id = format!("u_{}", super::random_hex(12));
    tx.execute(
        &format!(
            "INSERT INTO core_users ({COLUMNS}) VALUES ($1, $2, $3, $4, $5, NULL, NULL, 0, $6, $7)"
        ),
        &[
            id.as_str().into(),
            email.as_str().into(),
            user.name.trim().into(),
            user.role.name().into(),
            user.state.name().into(),
            user.expires_at.into(),
            now_ms.into(),
        ],
    )
    .await?;
    Ok(User {
        id,
        email,
        name: user.name.trim().to_string(),
        role: user.role,
        state: user.state,
        language: None,
        last_app: None,
        chooser_seen: false,
        expires_at: user.expires_at,
        created_at: now_ms,
    })
}

pub async fn create(store: &dyn AsyncStore, user: &NewUser<'_>, now_ms: i64) -> StoreResult<User> {
    let mut tx = store.begin().await?;
    let created = create_in(&mut *tx, user, now_ms).await?;
    tx.commit().await?;
    Ok(created)
}

pub async fn by_id(store: &dyn AsyncStore, id: &str) -> StoreResult<Option<User>> {
    let rows = store
        .query(
            &format!("SELECT {COLUMNS} FROM core_users WHERE id = $1"),
            &[id.into()],
        )
        .await?;
    rows.first().map(from_row).transpose()
}

pub async fn by_email(store: &dyn AsyncStore, email: &str) -> StoreResult<Option<User>> {
    let Some(email) = normal_email(email) else {
        return Ok(None);
    };
    let rows = store
        .query(
            &format!("SELECT {COLUMNS} FROM core_users WHERE email = $1"),
            &[email.into()],
        )
        .await?;
    rows.first().map(from_row).transpose()
}

/// Every account, by name.
pub async fn list(store: &dyn AsyncStore) -> StoreResult<Vec<User>> {
    store
        .query(
            &format!("SELECT {COLUMNS} FROM core_users ORDER BY name, email"),
            &[],
        )
        .await?
        .iter()
        .map(from_row)
        .collect()
}

pub async fn count(store: &dyn AsyncStore) -> StoreResult<i64> {
    let rows = store.query("SELECT COUNT(*) FROM core_users", &[]).await?;
    Ok(rows.first().and_then(|r| r.int(0)).unwrap_or_default())
}

/// Changes one field of an account.
pub async fn set(store: &dyn AsyncStore, id: &str, field: Field<'_>) -> StoreResult<()> {
    let (column, value): (&str, Value) = match field {
        Field::Name(name) => ("name", name.trim().into()),
        Field::Role(role) => ("role", role.name().into()),
        Field::State(state) => ("state", state.name().into()),
        Field::Language(language) => ("language", language.into()),
        Field::LastApp(app) => ("last_app", app.into()),
        Field::ChooserSeen => ("chooser_seen", 1i64.into()),
        Field::ExpiresAt(at) => ("expires_at", at.into()),
    };
    store
        .execute(
            &format!("UPDATE core_users SET {column} = $1 WHERE id = $2"),
            &[value, id.into()],
        )
        .await?;
    Ok(())
}

/// One field of an account that can change.
pub enum Field<'a> {
    Name(&'a str),
    Role(Role),
    State(State),
    Language(Option<&'a str>),
    LastApp(Option<&'a str>),
    ChooserSeen,
    ExpiresAt(Option<i64>),
}
