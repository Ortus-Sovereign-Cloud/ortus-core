//! The sign-in policy an owner sets: whether passwords (always with a
//! one-time code) are allowed, and how long sessions last. Passkeys are
//! always allowed; outside providers are on while they are configured.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sessions::Lifetime;
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "settings",
    sql: "CREATE TABLE core_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        changed_at BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.settings",
    migrations: MIGRATIONS,
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Password with a one-time code.
    pub password: bool,
    pub session_idle_days: i64,
    pub session_max_days: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            password: true,
            session_idle_days: 14,
            session_max_days: 90,
        }
    }
}

impl Policy {
    pub fn lifetime(&self) -> Lifetime {
        Lifetime {
            idle_ms: self.session_idle_days * DAY_MS,
            max_ms: self.session_max_days * DAY_MS,
        }
    }
}

/// One installation-wide setting, if it was ever set.
pub async fn setting(store: &dyn AsyncStore, key: &str) -> StoreResult<Option<String>> {
    let rows = store
        .query(
            "SELECT value FROM core_settings WHERE key = $1",
            &[key.into()],
        )
        .await?;
    Ok(rows.first().and_then(|r| r.text(0)).map(str::to_string))
}

/// Sets one installation-wide setting.
pub async fn set_setting(store: &dyn AsyncStore, key: &str, value: &str) -> StoreResult<()> {
    let mut tx = store.begin().await?;
    tx.execute("DELETE FROM core_settings WHERE key = $1", &[key.into()])
        .await?;
    tx.execute(
        "INSERT INTO core_settings (key, value, changed_at) VALUES ($1, $2, $3)",
        &[key.into(), value.into(), now_ms().into()],
    )
    .await?;
    tx.commit().await
}

pub async fn current(store: &dyn AsyncStore) -> StoreResult<Policy> {
    Ok(setting(store, "sign_in_policy")
        .await?
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default())
}

/// How long sessions last now (the defaults when the policy cannot be read).
pub async fn lifetime(store: &dyn AsyncStore) -> Lifetime {
    current(store).await.unwrap_or_default().lifetime()
}

async fn save(store: &dyn AsyncStore, policy: &Policy) -> StoreResult<()> {
    set_setting(
        store,
        "sign_in_policy",
        &serde_json::to_string(policy).unwrap_or_default(),
    )
    .await
}

/// The people who could not sign in without passwords: active, signing
/// in with a password and authenticator app, and nothing else.
async fn password_only(store: &dyn AsyncStore) -> StoreResult<Vec<String>> {
    let rows = store
        .query(
            "SELECT u.name FROM core_users u WHERE u.state = 'active'
             AND EXISTS (SELECT 1 FROM core_credentials c WHERE c.user_id = u.id AND c.kind = 'password')
             AND EXISTS (SELECT 1 FROM core_credentials c WHERE c.user_id = u.id AND c.kind = 'totp')
             AND NOT EXISTS (SELECT 1 FROM core_credentials c WHERE c.user_id = u.id AND c.kind = 'passkey')
             AND NOT EXISTS (SELECT 1 FROM core_external_links l WHERE l.user_id = u.id)
             ORDER BY u.name",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.text(0).map(str::to_string))
        .collect())
}

async fn count(
    store: &dyn AsyncStore,
    sql: &str,
    params: &[crate::storage::Value],
) -> StoreResult<i64> {
    Ok(store
        .query(sql, params)
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0))
}

async fn show(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let store = ortus.store.as_ref();
    let day = now_ms() - DAY_MS;
    let recent = crate::audit::list(
        store,
        &crate::audit::Filter {
            action_prefix: Some("auth.sign_in".into()),
            limit: 20,
            ..Default::default()
        },
    )
    .await?;
    let mut names = std::collections::HashMap::new();
    for user in super::accounts::list(store).await? {
        names.insert(user.id.clone(), user.name.clone());
    }
    let recent: Vec<Value> = recent
        .iter()
        .map(|e| {
            let detail: Value = serde_json::from_str(&e.detail).unwrap_or_default();
            json!({
                "at": e.at, "outcome": e.outcome, "address": e.address,
                "name": e.actor.as_ref().and_then(|a| names.get(a)).cloned().or_else(|| detail["email"].as_str().map(str::to_string)),
                "method": detail["methods"].as_str().or(detail["method"].as_str()),
            })
        })
        .collect();
    Ok(ok(&json!({
        "policy": current(store).await?,
        "password_only": password_only(store).await?,
        "stats": {
            "sign_ins_today": count(store, "SELECT COUNT(*) FROM core_audit WHERE action = 'auth.sign_in' AND outcome = 'done' AND at >= $1", &[day.into()]).await?,
            "failed_today": count(store, "SELECT COUNT(*) FROM core_audit WHERE action = 'auth.sign_in' AND outcome <> 'done' AND at >= $1", &[day.into()]).await?,
            "people_with_passkeys": count(store, "SELECT COUNT(DISTINCT user_id) FROM core_credentials WHERE kind = 'passkey'", &[]).await?,
            "people": count(store, "SELECT COUNT(*) FROM core_users WHERE state = 'active'", &[]).await?,
            "active_sessions": count(store, "SELECT COUNT(*) FROM core_sessions WHERE expires_at > $1", &[now_ms().into()]).await?,
        },
        "recent": recent,
    })))
}

#[derive(Deserialize)]
struct Change {
    password: bool,
    session_idle_days: i64,
    session_max_days: i64,
}

async fn change(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: Change = body(request)?;
    if !(1..=365).contains(&input.session_idle_days)
        || !(1..=365).contains(&input.session_max_days)
        || input.session_idle_days > input.session_max_days
    {
        return Err(ApiError::bad_request(
            "Sessions last 1 to 365 days, and idle time is not longer than the most.",
        ));
    }
    let store = ortus.store.as_ref();
    if !input.password {
        let stuck = password_only(store).await?;
        if !stuck.is_empty() {
            return Err(ApiError {
                status: 409,
                code: "would_lock_out",
                message: format!("{} people sign in with a password only: {}. Passwords stay on until they add a passkey.", stuck.len(), stuck.join(", ")),
            });
        }
    }
    let policy = Policy {
        password: input.password,
        session_idle_days: input.session_idle_days,
        session_max_days: input.session_max_days,
    };
    save(store, &policy).await?;
    super::audit(
        ortus,
        crate::audit::Event::new("auth.policy_changed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!(policy)),
    )
    .await;
    show(request, ortus).await
}

fn h_show(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, show(&request, &ortus).await) })
}
fn h_change(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, change(&request, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Get, path: "/admin/sign-in", id: "getSignInPolicy", tag: "sign-in", summary: "The sign-in policy, who still depends on a password, today's numbers and recent sign-ins.", access: Access::Admin, response: any_object, handler: h_show },
        Route { method: Method::Put, path: "/admin/sign-in", id: "setSignInPolicy", tag: "sign-in", summary: "Set the sign-in policy: {password, session_idle_days, session_max_days}. Turning passwords off is refused while someone would be locked out.", access: Access::Owner, response: any_object, handler: h_change },
    ]
}
