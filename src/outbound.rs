//! Every host the program has called, and why: the evidence behind the
//! Sovereignty screen's "no calls to third-party clouds". The places that
//! reach outside (outside sign-in providers, the directory, the mail relay)
//! note each call here; nothing else in the program calls out.

use serde_json::{json, Value};

use crate::program::now_ms;
use crate::storage::{AsyncStore, AsyncTx, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "outbound calls",
    sql: "CREATE TABLE core_outbound (
        host TEXT NOT NULL,
        purpose TEXT NOT NULL,
        calls BIGINT NOT NULL,
        last_at BIGINT NOT NULL,
        PRIMARY KEY (host, purpose)
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.outbound",
    migrations: MIGRATIONS,
};

/// The host (and port) of a URL, or the text itself when it is a host.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

const UPSERT: &str = "INSERT INTO core_outbound (host, purpose, calls, last_at) VALUES ($1, $2, 1, $3)
    ON CONFLICT (host, purpose) DO UPDATE SET calls = core_outbound.calls + 1, last_at = excluded.last_at";

/// Notes a call to `url` for `purpose` (sign_in, directory, mail). A note
/// that cannot be written is not a reason to fail the call.
pub async fn note(store: &dyn AsyncStore, purpose: &str, url: &str) {
    if let Err(e) = store
        .execute(
            UPSERT,
            &[host_of(url).into(), purpose.into(), now_ms().into()],
        )
        .await
    {
        eprintln!("ortus: outbound: could not note a call: {e}");
    }
}

/// The same, inside a job's transaction.
pub async fn note_in(tx: &mut dyn AsyncTx, purpose: &str, url: &str) -> StoreResult<()> {
    tx.execute(
        UPSERT,
        &[host_of(url).into(), purpose.into(), now_ms().into()],
    )
    .await
    .map(|_| ())
}

/// Every host called, newest first.
pub async fn calls(store: &dyn AsyncStore) -> StoreResult<Vec<Value>> {
    Ok(store
        .query("SELECT host, purpose, calls, last_at FROM core_outbound ORDER BY last_at DESC", &[])
        .await?
        .iter()
        .map(|r| json!({ "host": r.text(0), "purpose": r.text(1), "calls": r.int(2), "last_at": r.int(3) }))
        .collect())
}
