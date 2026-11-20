//! The audit log: who did what, when, from where, and whether it worked.
//!
//! Events form a hash chain: each event's hash covers its own fields and
//! the hash of the event before it, so removing or changing an event
//! breaks every hash after it, and [`verify`] finds where. A one-row head
//! table serialises writers on both engines (its row lock on PostgreSQL,
//! the write lock on SQLite), so the chain has no forks.
//!
//! An event is written in the same transaction as the change it records
//! when there is one ([`record_in`]), so a change never happens unlogged.

use serde::Serialize;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use crate::storage::{AsyncStore, AsyncTx, Migration, Schema, StoreError, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "audit log",
    sql: "CREATE TABLE core_audit (
        seq BIGINT PRIMARY KEY,
        at BIGINT NOT NULL,
        actor TEXT,
        action TEXT NOT NULL,
        target TEXT,
        outcome TEXT NOT NULL,
        address TEXT,
        detail TEXT NOT NULL,
        hash TEXT NOT NULL
    );
    CREATE INDEX core_audit_actor ON core_audit (actor, seq);
    CREATE INDEX core_audit_action ON core_audit (action, seq);
    CREATE TABLE core_audit_head (
        id BIGINT PRIMARY KEY,
        seq BIGINT NOT NULL,
        hash TEXT NOT NULL
    );
    INSERT INTO core_audit_head (id, seq, hash) VALUES (1, 0, '')",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.audit",
    migrations: MIGRATIONS,
};

/// Whether what was tried happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Done,
    /// Refused by a rule (a label, a policy, a permission).
    Refused,
    /// Tried and failed (a wrong password, a bad code).
    Failed,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Refused => "refused",
            Outcome::Failed => "failed",
        }
    }
}

/// One thing that happened.
#[derive(Clone, Debug)]
pub struct Event<'a> {
    /// The account that acted, or `None` for someone not signed in or the
    /// system.
    pub actor: Option<&'a str>,
    /// What was done, as `auth.sign_in` or `drive.share`.
    pub action: &'a str,
    /// What it was done to, as an account or file id.
    pub target: Option<&'a str>,
    pub outcome: Outcome,
    /// The network address of the request, if any.
    pub address: Option<&'a str>,
    /// Anything else worth keeping, as JSON. Never secrets.
    pub detail: Json,
}

impl<'a> Event<'a> {
    pub fn new(action: &'a str, outcome: Outcome) -> Self {
        Self {
            actor: None,
            action,
            target: None,
            outcome,
            address: None,
            detail: json!({}),
        }
    }

    pub fn by(mut self, actor: &'a str) -> Self {
        self.actor = Some(actor);
        self
    }

    pub fn on(mut self, target: &'a str) -> Self {
        self.target = Some(target);
        self
    }

    pub fn from(mut self, address: &'a str) -> Self {
        self.address = Some(address);
        self
    }

    pub fn with(mut self, detail: Json) -> Self {
        self.detail = detail;
        self
    }
}

/// The hash of an event, chained to the one before it.
fn hash(previous: &str, seq: i64, at: i64, event: &Stored) -> String {
    let canonical = json!([
        previous,
        seq,
        at,
        event.actor,
        event.action,
        event.target,
        event.outcome,
        event.address,
        event.detail
    ]);
    hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stored {
    pub seq: i64,
    pub at: i64,
    pub actor: Option<String>,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
    pub address: Option<String>,
    /// The detail as JSON text, exactly as hashed.
    pub detail: String,
    pub hash: String,
}

/// Writes `event` in `tx`, after every event written before it.
pub async fn record_in(tx: &mut dyn AsyncTx, event: &Event<'_>, now_ms: i64) -> StoreResult<i64> {
    // Taking the head row first serialises every writer.
    tx.execute("UPDATE core_audit_head SET seq = seq + 1 WHERE id = 1", &[])
        .await?;
    let head = tx
        .query("SELECT seq, hash FROM core_audit_head WHERE id = 1", &[])
        .await?;
    let row = head
        .first()
        .ok_or_else(|| StoreError::Database("the audit head is missing".into()))?;
    let seq = row.int(0).unwrap_or_default();
    let previous = row.text(1).unwrap_or_default().to_string();
    let mut stored = Stored {
        seq,
        at: now_ms,
        actor: event.actor.map(str::to_string),
        action: event.action.to_string(),
        target: event.target.map(str::to_string),
        outcome: event.outcome.name().to_string(),
        address: event.address.map(str::to_string),
        detail: event.detail.to_string(),
        hash: String::new(),
    };
    stored.hash = hash(&previous, seq, now_ms, &stored);
    tx.execute(
        "INSERT INTO core_audit (seq, at, actor, action, target, outcome, address, detail, hash) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &[
            seq.into(),
            now_ms.into(),
            stored.actor.clone().into(),
            stored.action.as_str().into(),
            stored.target.clone().into(),
            stored.outcome.as_str().into(),
            stored.address.clone().into(),
            stored.detail.as_str().into(),
            stored.hash.as_str().into(),
        ],
    )
    .await?;
    tx.execute(
        "UPDATE core_audit_head SET hash = $1 WHERE id = 1",
        &[stored.hash.as_str().into()],
    )
    .await?;
    Ok(seq)
}

/// Writes `event` in a transaction of its own.
pub async fn record(store: &dyn AsyncStore, event: &Event<'_>, now_ms: i64) -> StoreResult<i64> {
    let mut tx = store.begin().await?;
    let seq = record_in(&mut *tx, event, now_ms).await?;
    tx.commit().await?;
    Ok(seq)
}

fn from_row(row: &crate::storage::Row) -> Stored {
    let text = |i: usize| row.text(i).map(str::to_string);
    Stored {
        seq: row.int(0).unwrap_or_default(),
        at: row.int(1).unwrap_or_default(),
        actor: text(2),
        action: text(3).unwrap_or_default(),
        target: text(4),
        outcome: text(5).unwrap_or_default(),
        address: text(6),
        detail: text(7).unwrap_or_default(),
        hash: text(8).unwrap_or_default(),
    }
}

const COLUMNS: &str = "seq, at, actor, action, target, outcome, address, detail, hash";

/// A filter for listing events.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub actor: Option<String>,
    pub action_prefix: Option<String>,
    /// Only events before this one (for paging back).
    pub before: Option<i64>,
    pub limit: i64,
}

/// Events, newest first.
pub async fn list(store: &dyn AsyncStore, filter: &Filter) -> StoreResult<Vec<Stored>> {
    let mut sql = format!("SELECT {COLUMNS} FROM core_audit WHERE 1 = 1");
    let mut params: Vec<crate::storage::Value> = Vec::new();
    if let Some(actor) = &filter.actor {
        params.push(actor.as_str().into());
        sql.push_str(&format!(" AND actor = ${}", params.len()));
    }
    if let Some(prefix) = &filter.action_prefix {
        // A prefix match without LIKE (its escaping differs by engine)
        // and without ranges (their order depends on the collation).
        params.push(prefix.as_str().into());
        params.push((prefix.chars().count() as i64).into());
        let n = params.len();
        sql.push_str(&format!(" AND substr(action, 1, ${n}) = ${}", n - 1));
    }
    if let Some(before) = filter.before {
        params.push(before.into());
        sql.push_str(&format!(" AND seq < ${}", params.len()));
    }
    params.push(filter.limit.clamp(1, 500).into());
    sql.push_str(&format!(" ORDER BY seq DESC LIMIT ${}", params.len()));
    Ok(store
        .query(&sql, &params)
        .await?
        .iter()
        .map(from_row)
        .collect())
}

/// The result of checking the chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Verified {
    pub events: i64,
    /// The first event whose hash does not follow, if any.
    pub broken_at: Option<i64>,
}

/// Recomputes every hash in order and reports the first break: an event
/// changed, removed (a gap) or added out of order.
pub async fn verify(store: &dyn AsyncStore) -> StoreResult<Verified> {
    let mut previous = String::new();
    let mut expected_seq = 1;
    let mut after = 0i64;
    loop {
        let rows = store
            .query(
                &format!("SELECT {COLUMNS} FROM core_audit WHERE seq > $1 ORDER BY seq LIMIT 1000"),
                &[after.into()],
            )
            .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let event = from_row(row);
            let mut check = event.clone();
            check.hash.clear();
            if event.seq != expected_seq
                || hash(&previous, event.seq, event.at, &check) != event.hash
            {
                return Ok(Verified {
                    events: expected_seq - 1,
                    broken_at: Some(expected_seq),
                });
            }
            previous = event.hash;
            expected_seq += 1;
            after = event.seq;
        }
    }
    let head = store
        .query("SELECT seq, hash FROM core_audit_head WHERE id = 1", &[])
        .await?;
    let head_seq = head.first().and_then(|r| r.int(0)).unwrap_or_default();
    let head_hash = head
        .first()
        .and_then(|r| r.text(1))
        .unwrap_or_default()
        .to_string();
    // Events cut from the end show as a head ahead of the last event.
    if head_seq != expected_seq - 1 || head_hash != previous {
        return Ok(Verified {
            events: expected_seq - 1,
            broken_at: Some(expected_seq),
        });
    }
    Ok(Verified {
        events: expected_seq - 1,
        broken_at: None,
    })
}

/// The value of `name` in a request's query string, percent-decoded.
pub fn query_param(uri: &str, name: &str) -> Option<String> {
    let query = uri.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| decode(value))
    })
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                // Bytes, not a str slice: raw UTF-8 after % must not panic.
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                    .ok_or(())
                {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

mod routes {
    use std::sync::Arc;

    use rust_web_server::prelude::*;
    use serde_json::{json, Value};

    use super::{list, query_param, verify, Filter};
    use crate::api::{ok, ApiError, Method, Route};
    use crate::auth::Access;
    use crate::instance::Instance;
    use crate::storage::BoxFuture;

    fn events(
        request: Request,
        _: PathParams,
        _: ConnectionInfo,
        ortus: Arc<Instance>,
    ) -> BoxFuture<'static, Response> {
        Box::pin(async move {
            let uri = request.request_uri.clone();
            let filter = Filter {
                actor: query_param(&uri, "actor").filter(|v| !v.is_empty()),
                action_prefix: query_param(&uri, "action").filter(|v| !v.is_empty()),
                before: query_param(&uri, "before").and_then(|v| v.parse().ok()),
                limit: query_param(&uri, "limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(100),
            };
            match list(ortus.store.as_ref(), &filter).await {
                Ok(events) => ok(&json!({ "events": events })),
                Err(_) => ApiError::internal().response(&request),
            }
        })
    }

    fn check(
        request: Request,
        _: PathParams,
        _: ConnectionInfo,
        ortus: Arc<Instance>,
    ) -> BoxFuture<'static, Response> {
        Box::pin(async move {
            match verify(ortus.store.as_ref()).await {
                Ok(result) => ok(&json!(result)),
                Err(_) => ApiError::internal().response(&request),
            }
        })
    }

    fn any_object() -> Value {
        json!({"type": "object"})
    }

    pub fn routes() -> Vec<Route> {
        vec![
            Route { method: Method::Get, path: "/admin/audit", id: "listAuditEvents", tag: "audit", summary: "Audit events, newest first. Query: actor, action (a prefix, as auth.), before (a seq), limit (up to 500).", access: Access::Admin, response: any_object, handler: events },
            Route { method: Method::Get, path: "/admin/audit/verify", id: "verifyAuditLog", tag: "audit", summary: "Recompute the hash chain of the audit log and report the first break, if any.", access: Access::Admin, response: any_object, handler: check },
        ]
    }
}

pub use routes::routes;

#[cfg(test)]
mod tests {
    use super::query_param;

    #[test]
    fn query_values_are_decoded_safely() {
        assert_eq!(
            Some("auth.".to_string()),
            query_param("/x?action=auth.&limit=5", "action")
        );
        assert_eq!(Some("a b/c".to_string()), query_param("/x?q=a+b%2Fc", "q"));
        assert_eq!(Some("%é".to_string()), query_param("/x?q=%é", "q"));
        assert_eq!(Some("%".to_string()), query_param("/x?q=%", "q"));
        assert_eq!(None, query_param("/x", "q"));
    }
}
