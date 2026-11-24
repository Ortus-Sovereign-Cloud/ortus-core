//! Sessions: a signed-in browser or app holds a random token; the server
//! keeps only its SHA-256, so a copy of the database signs no one in.

use sha2::{Digest, Sha256};

use super::accounts::{self, State, User};
use crate::storage::{AsyncStore, StoreResult};

/// How long a session lasts without being used, and at most.
#[derive(Clone, Copy, Debug)]
pub struct Lifetime {
    pub idle_ms: i64,
    pub max_ms: i64,
}

impl Default for Lifetime {
    fn default() -> Self {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        Self {
            idle_ms: 14 * DAY,
            max_ms: 90 * DAY,
        }
    }
}

/// A signed-in session, with its account.
#[derive(Clone, Debug)]
pub struct Session {
    pub token_hash: String,
    pub user: User,
    /// How the person proved who they are, as `passkey` or `password+totp`.
    pub methods: String,
    pub created_at: i64,
    pub expires_at: i64,
}

pub fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Starts a session for `user` and returns its token, for the cookie.
pub async fn start(
    store: &dyn AsyncStore,
    user_id: &str,
    methods: &str,
    address: &str,
    agent: &str,
    lifetime: Lifetime,
    now_ms: i64,
) -> StoreResult<String> {
    let token = super::random_token();
    store
        .execute(
            "INSERT INTO core_sessions (token_hash, user_id, methods, created_at, last_seen_at, expires_at, address, agent) VALUES ($1, $2, $3, $4, $4, $5, $6, $7)",
            &[
                hash(&token).into(),
                user_id.into(),
                methods.into(),
                now_ms.into(),
                (now_ms + lifetime.idle_ms.min(lifetime.max_ms)).into(),
                address.into(),
                agent.chars().take(300).collect::<String>().into(),
            ],
        )
        .await?;
    Ok(token)
}

/// The session for `token`, when it is current and its account active.
/// Using a session moves its idle expiry on (at most once a minute).
pub async fn find(
    store: &dyn AsyncStore,
    token: &str,
    lifetime: Lifetime,
    now_ms: i64,
) -> StoreResult<Option<Session>> {
    find_with(store, token, Some(lifetime), now_ms).await
}

/// As [`find`], with the lifetimes of the sign-in policy, read only when
/// the session's expiry moves on (not on every request).
pub async fn find_current(
    store: &dyn AsyncStore,
    token: &str,
    now_ms: i64,
) -> StoreResult<Option<Session>> {
    find_with(store, token, None, now_ms).await
}

async fn find_with(
    store: &dyn AsyncStore,
    token: &str,
    lifetime: Option<Lifetime>,
    now_ms: i64,
) -> StoreResult<Option<Session>> {
    let token_hash = hash(token);
    let rows = store
        .query(
            "SELECT user_id, methods, created_at, last_seen_at, expires_at FROM core_sessions WHERE token_hash = $1",
            &[token_hash.as_str().into()],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let (created_at, last_seen_at, expires_at) = (
        row.int(2).unwrap_or_default(),
        row.int(3).unwrap_or_default(),
        row.int(4).unwrap_or_default(),
    );
    if expires_at <= now_ms {
        end(store, token).await?;
        return Ok(None);
    }
    let Some(user) = accounts::by_id(store, row.text(0).unwrap_or_default()).await? else {
        return Ok(None);
    };
    if user.state != State::Active || user.expires_at.is_some_and(|at| at <= now_ms) {
        return Ok(None);
    }
    let mut expires_at = expires_at;
    if now_ms - last_seen_at > 60_000 {
        let lifetime = match lifetime {
            Some(lifetime) => lifetime,
            None => super::policy::lifetime(store).await,
        };
        expires_at = (now_ms + lifetime.idle_ms).min(created_at + lifetime.max_ms);
        store
            .execute(
                "UPDATE core_sessions SET last_seen_at = $1, expires_at = $2 WHERE token_hash = $3",
                &[now_ms.into(), expires_at.into(), token_hash.as_str().into()],
            )
            .await?;
    }
    Ok(Some(Session {
        token_hash,
        user,
        methods: row.text(1).unwrap_or_default().to_string(),
        created_at,
        expires_at,
    }))
}

/// Ends the session for `token` (signing out).
pub async fn end(store: &dyn AsyncStore, token: &str) -> StoreResult<()> {
    crate::request::forget_session();
    store
        .execute(
            "DELETE FROM core_sessions WHERE token_hash = $1",
            &[hash(token).into()],
        )
        .await?;
    Ok(())
}

/// Ends every session of an account, as after a suspension or a lost
/// device.
pub async fn end_all(store: &dyn AsyncStore, user_id: &str) -> StoreResult<u64> {
    crate::request::forget_session();
    store
        .execute(
            "DELETE FROM core_sessions WHERE user_id = $1",
            &[user_id.into()],
        )
        .await
}

/// A session as listed to its owner or an administrator.
#[derive(Clone, Debug)]
pub struct Listed {
    pub token_hash: String,
    pub methods: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub address: String,
    pub agent: String,
}

pub async fn list(store: &dyn AsyncStore, user_id: &str) -> StoreResult<Vec<Listed>> {
    Ok(store
        .query(
            "SELECT token_hash, methods, created_at, last_seen_at, address, agent FROM core_sessions WHERE user_id = $1 ORDER BY last_seen_at DESC",
            &[user_id.into()],
        )
        .await?
        .iter()
        .map(|row| Listed {
            token_hash: row.text(0).unwrap_or_default().into(),
            methods: row.text(1).unwrap_or_default().into(),
            created_at: row.int(2).unwrap_or_default(),
            last_seen_at: row.int(3).unwrap_or_default(),
            address: row.text(4).unwrap_or_default().into(),
            agent: row.text(5).unwrap_or_default().into(),
        })
        .collect())
}
