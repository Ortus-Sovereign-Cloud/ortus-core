//! The first start: an installation without accounts makes a one-time
//! setup token and prints a link with it. Whoever opens the link creates
//! the first owner. Nobody can do that without the link, and it works
//! once.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::accounts::{self, NewUser, Role, State, User};
use crate::instance::Instance;
use crate::storage::{AsyncStore, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "setup token",
    sql: "CREATE TABLE core_setup (
        id BIGINT PRIMARY KEY,
        token_hash TEXT NOT NULL,
        created_at BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.setup",
    migrations: MIGRATIONS,
};

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Whether the installation still needs its first owner. Other accounts
/// (an early sign-up) do not count: only an owner ends setup.
pub async fn needed(store: &dyn AsyncStore) -> StoreResult<bool> {
    let rows = store
        .query("SELECT COUNT(*) FROM core_users WHERE role = 'owner'", &[])
        .await?;
    Ok(rows.first().and_then(|r| r.int(0)).unwrap_or_default() == 0)
}

/// Makes the setup token when the installation has no account and no
/// token yet, and returns the link to print. `fixed` is a token chosen in
/// the settings (`ORTUS_SETUP_TOKEN`), for automated installs. With
/// `replace`, an existing token is replaced (the `setup-link` command).
pub async fn prepare(
    ortus: &Arc<Instance>,
    fixed: Option<&str>,
    replace: bool,
    now_ms: i64,
) -> StoreResult<Option<String>> {
    let store = ortus.store.as_ref();
    if !needed(store).await? {
        store.execute("DELETE FROM core_setup", &[]).await?;
        return Ok(None);
    }
    let token = fixed.map_or_else(super::random_token, str::to_string);
    let mut tx = store.begin().await?;
    let existing = tx.query("SELECT COUNT(*) FROM core_setup", &[]).await?;
    if existing.first().and_then(|r| r.int(0)).unwrap_or_default() > 0
        && !replace
        && fixed.is_none()
    {
        tx.rollback().await?;
        return Ok(None);
    }
    tx.execute("DELETE FROM core_setup", &[]).await?;
    tx.execute(
        "INSERT INTO core_setup (id, token_hash, created_at) VALUES (1, $1, $2)",
        &[hash(&token).into(), now_ms.into()],
    )
    .await?;
    tx.commit().await?;
    Ok(Some(format!("{}/setup#{token}", ortus.config.public_url)))
}

/// Why the first owner was not created.
#[derive(Debug, PartialEq, Eq)]
pub enum SetupError {
    /// The installation already has its owner.
    Done,
    /// The token is not the one in the setup link.
    BadToken,
    BadEmail,
    BadName,
    Store(String),
}

/// Creates the first owner with the setup token. The token is used up.
pub async fn create_owner(
    store: &dyn AsyncStore,
    token: &str,
    email: &str,
    name: &str,
    now_ms: i64,
) -> Result<User, SetupError> {
    if accounts::normal_email(email).is_none() {
        return Err(SetupError::BadEmail);
    }
    if name.trim().is_empty() || name.len() > 200 {
        return Err(SetupError::BadName);
    }
    let store_error = |e: crate::storage::StoreError| SetupError::Store(e.to_string());
    let mut tx = store.begin().await.map_err(store_error)?;
    let users = tx
        .query("SELECT COUNT(*) FROM core_users WHERE role = 'owner'", &[])
        .await
        .map_err(store_error)?;
    if users.first().and_then(|r| r.int(0)).unwrap_or_default() > 0 {
        return Err(SetupError::Done);
    }
    let rows = tx
        .query("SELECT token_hash FROM core_setup WHERE id = 1", &[])
        .await
        .map_err(store_error)?;
    let Some(expected) = rows.first().and_then(|r| r.text(0)).map(str::to_string) else {
        return Err(SetupError::BadToken);
    };
    // Both sides are hashes of the same length: compare in constant time.
    let given = hash(token);
    let same = given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !same {
        return Err(SetupError::BadToken);
    }
    let user = accounts::create_in(
        &mut *tx,
        &NewUser {
            email,
            name,
            role: Role::Owner,
            state: State::Active,
            expires_at: None,
        },
        now_ms,
    )
    .await
    .map_err(store_error)?;
    tx.execute("DELETE FROM core_setup", &[])
        .await
        .map_err(store_error)?;
    tx.commit().await.map_err(store_error)?;
    Ok(user)
}
