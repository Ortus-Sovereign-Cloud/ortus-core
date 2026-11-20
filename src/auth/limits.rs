//! Slows down guessing: after a few failed sign-ins for an address or from
//! a network address, further tries wait, longer each time (up to 15
//! minutes). A success clears the count.

use crate::storage::{AsyncStore, StoreResult};

/// Failures allowed before waiting starts.
const FREE: i64 = 5;
const MAX_WAIT_MS: i64 = 15 * 60 * 1000;

/// When the key may try again, if it must wait.
pub async fn blocked_until(
    store: &dyn AsyncStore,
    key: &str,
    now_ms: i64,
) -> StoreResult<Option<i64>> {
    let rows = store
        .query(
            "SELECT blocked_until FROM core_signin_limits WHERE key = $1",
            &[key.into()],
        )
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.int(0))
        .filter(|until| *until > now_ms))
}

/// Counts a failure for each key.
pub async fn failed(store: &dyn AsyncStore, keys: &[String], now_ms: i64) -> StoreResult<()> {
    for key in keys {
        let mut tx = store.begin().await?;
        let rows = tx
            .query(
                "SELECT failures FROM core_signin_limits WHERE key = $1",
                &[key.as_str().into()],
            )
            .await?;
        let failures = rows.first().and_then(|r| r.int(0)).unwrap_or(0) + 1;
        let wait = if failures < FREE {
            0
        } else {
            (30_000i64 << (failures - FREE).min(10)).min(MAX_WAIT_MS)
        };
        tx.execute(
            "DELETE FROM core_signin_limits WHERE key = $1",
            &[key.as_str().into()],
        )
        .await?;
        tx.execute(
            "INSERT INTO core_signin_limits (key, failures, blocked_until) VALUES ($1, $2, $3)",
            &[key.as_str().into(), failures.into(), (now_ms + wait).into()],
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

pub async fn succeeded(store: &dyn AsyncStore, keys: &[String]) -> StoreResult<()> {
    for key in keys {
        store
            .execute(
                "DELETE FROM core_signin_limits WHERE key = $1",
                &[key.as_str().into()],
            )
            .await?;
    }
    Ok(())
}

/// The keys of a sign-in attempt: the address tried and where from.
pub fn keys(email: &str, client_ip: &str) -> Vec<String> {
    vec![
        format!("email:{}", email.trim().to_lowercase()),
        format!("ip:{client_ip}"),
    ]
}
