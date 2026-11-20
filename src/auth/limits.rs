//! Slows down guessing: after a few failed sign-ins for an address or from
//! a network address, further tries wait, longer each time (up to 15
//! minutes). A success clears the count.

use crate::storage::{AsyncStore, StoreResult};

/// Failures allowed before waiting starts.
const FREE: i64 = 5;
const MAX_WAIT_MS: i64 = 15 * 60 * 1000;

/// When the key may try again, if it must wait.
/// Takes one try for `key`, unless it is blocked: the try counts as a
/// failure at once (a success clears it), in one conditional statement, so
/// parallel requests cannot all pass a check made before any failure was
/// written. Answers when the block ends if the key is blocked.
pub async fn blocked_until(
    store: &dyn AsyncStore,
    key: &str,
    now_ms: i64,
) -> StoreResult<Option<i64>> {
    store
        .execute(
            "INSERT INTO core_signin_limits (key, failures, blocked_until) VALUES ($1, 0, 0) ON CONFLICT (key) DO NOTHING",
            &[key.into()],
        )
        .await?;
    let taken = store
        .execute(
            &format!(
                "UPDATE core_signin_limits SET failures = failures + 1, blocked_until = CASE
                    WHEN failures + 1 < {FREE} THEN blocked_until
                    WHEN failures + 1 = {FREE} THEN $2 + 30000
                    WHEN failures + 1 = {FREE} + 1 THEN $2 + 60000
                    WHEN failures + 1 = {FREE} + 2 THEN $2 + 120000
                    WHEN failures + 1 = {FREE} + 3 THEN $2 + 240000
                    WHEN failures + 1 = {FREE} + 4 THEN $2 + 480000
                    ELSE $2 + {MAX_WAIT_MS} END
                 WHERE key = $1 AND blocked_until <= $2"
            ),
            &[key.into(), now_ms.into()],
        )
        .await?;
    if taken == 1 {
        return Ok(None);
    }
    let rows = store
        .query(
            "SELECT blocked_until FROM core_signin_limits WHERE key = $1",
            &[key.into()],
        )
        .await?;
    Ok(Some(
        rows.first().and_then(|r| r.int(0)).unwrap_or(now_ms + 1000),
    ))
}

/// A failed try. It was counted when it was taken (`blocked_until`).
pub async fn failed(_store: &dyn AsyncStore, _keys: &[String], _now_ms: i64) -> StoreResult<()> {
    Ok(())
}

/// Gives back the try just taken (a step that went right but is not the
/// end of signing in). Earlier failures stay counted.
pub async fn refund(store: &dyn AsyncStore, keys: &[String]) -> StoreResult<()> {
    for key in keys {
        store
            .execute("UPDATE core_signin_limits SET failures = failures - 1 WHERE key = $1 AND failures > 0", &[key.as_str().into()])
            .await?;
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

/// Where tries come from, as one: an IPv4 address, or an IPv6 /64 (one
/// network hands out a whole /64).
pub fn network(client_ip: &str) -> String {
    match client_ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        Ok(std::net::IpAddr::V6(v6)) => v6
            .to_ipv4_mapped()
            .map_or(client_ip.to_string(), |v4| v4.to_string()),
        _ => client_ip.to_string(),
    }
}

/// The keys of a sign-in attempt: the address tried and where from.
pub fn keys(email: &str, client_ip: &str) -> Vec<String> {
    let from = network(client_ip);
    vec![
        // The address together with where the tries come from: a stranger
        // guessing someone's password slows down without locking that
        // person out of their own sign-in.
        format!("email:{}|{from}", email.trim().to_lowercase()),
        format!("ip:{from}"),
    ]
}

/// Tries on one account from anywhere, counted without ever blocking it:
/// past a few dozen, each try waits a little longer (up to five seconds),
/// so a guesser spread over many networks stays slow.
pub async fn account_delay(
    store: &dyn AsyncStore,
    email: &str,
) -> StoreResult<std::time::Duration> {
    let key = format!("account:{}", email.trim().to_lowercase());
    store
        .execute(
            "INSERT INTO core_signin_limits (key, failures, blocked_until) VALUES ($1, 0, 0) ON CONFLICT (key) DO NOTHING",
            &[key.as_str().into()],
        )
        .await?;
    store
        .execute(
            "UPDATE core_signin_limits SET failures = failures + 1 WHERE key = $1",
            &[key.as_str().into()],
        )
        .await?;
    let tries = store
        .query(
            "SELECT failures FROM core_signin_limits WHERE key = $1",
            &[key.as_str().into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    Ok(std::time::Duration::from_millis(
        ((tries - 30).max(0) * 250).min(5000) as u64,
    ))
}

/// The account's count starts again (a sign-in went right).
pub async fn account_ok(store: &dyn AsyncStore, email: &str) -> StoreResult<()> {
    store
        .execute(
            "DELETE FROM core_signin_limits WHERE key = $1",
            &[format!("account:{}", email.trim().to_lowercase()).into()],
        )
        .await?;
    Ok(())
}
