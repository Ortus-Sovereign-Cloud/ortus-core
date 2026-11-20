//! Outgoing webhooks: other systems (a case management system, a records
//! archive) hear of what happens in Ortus. An owner adds a hook with an
//! address and the events it wants; Ortus posts each event as JSON, signed
//! with the hook's secret, and tries again with growing pauses when the
//! other side does not take it. Every change to hooks is audited.
//!
//! Events come from the audit log (every app already writes there), read
//! from where the last run stopped; a hook hears only events after it was
//! added. A post carries:
//!
//! ```text
//! X-Ortus-Event: record.filed
//! X-Ortus-Delivery: <id>
//! X-Ortus-Timestamp: <Unix seconds>
//! X-Ortus-Signature: sha256=<hex HMAC-SHA256 of "<timestamp>.<body>">
//! {"event": "record.filed", "id": 1234, "at": <ms>, "target": "...", "detail": {...}}
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::auth::Access;
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "outgoing webhooks",
    sql: "CREATE TABLE core_webhooks (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        url TEXT NOT NULL,
        events TEXT NOT NULL,
        secret TEXT NOT NULL,
        created_by TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        from_seq BIGINT NOT NULL,
        last_ok_at BIGINT,
        last_error TEXT
    );
    CREATE TABLE core_webhook_deliveries (
        id TEXT PRIMARY KEY,
        hook TEXT NOT NULL,
        event TEXT NOT NULL,
        body TEXT NOT NULL,
        attempts BIGINT NOT NULL,
        next_at BIGINT NOT NULL,
        state TEXT NOT NULL,
        last_error TEXT,
        at BIGINT NOT NULL
    );
    CREATE INDEX core_webhook_deliveries_due ON core_webhook_deliveries (state, next_at);
    CREATE TABLE core_webhook_cursor (
        id BIGINT PRIMARY KEY,
        seq BIGINT NOT NULL
    );
    INSERT INTO core_webhook_cursor (id, seq) VALUES (1, 0)",
},
Migration {
    version: 2,
    name: "webhooks start from the audit log's end",
    sql: "UPDATE core_webhook_cursor SET seq = (SELECT COALESCE(MAX(seq), 0) FROM core_audit) WHERE id = 1",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.webhooks",
    migrations: MIGRATIONS,
};

/// The job that hands out and posts events (every minute).
pub const JOB: &str = "core.webhooks";

/// The events a hook can want: its public name, and the audit action it
/// comes from.
pub const EVENTS: &[(&str, &str)] = &[
    ("record.filed", "records.filed"),
    ("approval.done", "docs.review_done"),
    ("document.signed", "docs.signed"),
    ("task.created", "tasks.created"),
    ("person.invited", "people.invited"),
    ("person.joined", "people.invitation_accepted"),
    ("person.removed", "auth.account_deleted"),
    ("backup.restored", "backup.restored"),
];

/// Tries before a delivery is given up.
const ATTEMPTS: i64 = 8;
/// Hooks one installation keeps.
const MAX_HOOKS: i64 = 50;

fn public_name(action: &str) -> Option<&'static str> {
    EVENTS.iter().find(|(_, a)| *a == action).map(|(n, _)| *n)
}

fn sign(secret: &str, timestamp: i64, body: &str) -> String {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret.as_bytes());
    let tag = aws_lc_rs::hmac::sign(&key, format!("{timestamp}.{body}").as_bytes());
    format!("sha256={}", hex::encode(tag.as_ref()))
}

/// Hands new audit events to the hooks that want them (as deliveries).
async fn hand_out(instance: &Instance) -> Result<(), String> {
    // Up to 20 batches of 500 events a run: a backlog drains in minutes.
    for _ in 0..20 {
        if !hand_out_batch(instance).await? {
            break;
        }
    }
    Ok(())
}

/// One batch, in one transaction: the cursor, the events after it and the
/// hooks as they are now are read together, and the cursor moves only if
/// nobody else moved it meanwhile (two workers queue an event once).
/// True when there may be more.
async fn hand_out_batch(instance: &Instance) -> Result<bool, String> {
    let s = instance.store.as_ref();
    let e = |e: crate::storage::StoreError| e.to_string();
    let mut tx = s.begin().await.map_err(e)?;
    let cursor = tx
        .query("SELECT seq FROM core_webhook_cursor WHERE id = 1", &[])
        .await
        .map_err(e)?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    let rows = tx
        .query(
            "SELECT seq, at, action, target, detail FROM core_audit WHERE seq > $1 ORDER BY seq LIMIT 500",
            &[cursor.into()],
        )
        .await
        .map_err(e)?;
    let Some(last) = rows.last().and_then(|r| r.int(0)) else {
        tx.rollback().await.map_err(e)?;
        return Ok(false);
    };
    let hooks: Vec<(String, Vec<String>, i64)> = tx
        .query("SELECT id, events, from_seq FROM core_webhooks", &[])
        .await
        .map_err(e)?
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap_or_default().to_string(),
                serde_json::from_str(r.text(1).unwrap_or("[]")).unwrap_or_default(),
                r.int(2).unwrap_or(0),
            )
        })
        .collect();
    let now = now_ms();
    for r in &rows {
        let seq = r.int(0).unwrap_or(0);
        let Some(event) = r.text(2).and_then(public_name) else {
            continue;
        };
        let detail: Value =
            serde_json::from_str(r.text(4).unwrap_or("null")).unwrap_or(Value::Null);
        let body = json!({"event": event, "id": seq, "at": r.int(1), "target": r.text(3), "detail": detail}).to_string();
        for (hook, wanted, from) in &hooks {
            if seq <= *from || !wanted.iter().any(|w| w == event) {
                continue;
            }
            tx.execute(
                "INSERT INTO core_webhook_deliveries (id, hook, event, body, attempts, next_at, state, at) VALUES ($1, $2, $3, $4, 0, $5, 'queued', $5)",
                &[format!("d{}", crate::auth::random_hex(10)).into(), hook.as_str().into(), event.into(), body.as_str().into(), now.into()],
            )
            .await
            .map_err(e)?;
        }
    }
    let moved = tx
        .execute(
            "UPDATE core_webhook_cursor SET seq = $1 WHERE id = 1 AND seq = $2",
            &[last.into(), cursor.into()],
        )
        .await
        .map_err(e)?;
    if moved != 1 {
        tx.rollback().await.map_err(e)?;
        return Ok(false);
    }
    tx.commit().await.map_err(e)?;
    Ok(rows.len() == 500)
}

/// Posts one delivery; the error when the other side did not take it.
async fn post(
    url: &str,
    secret: &str,
    delivery: &str,
    event: &str,
    body: &str,
) -> Result<(), String> {
    let timestamp = now_ms() / 1000;
    let response = crate::http::client()?
        .post(url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "Ortus webhooks")
        .header("X-Ortus-Event", event)
        .header("X-Ortus-Delivery", delivery)
        .header("X-Ortus-Timestamp", timestamp.to_string())
        .header("X-Ortus-Signature", sign(secret, timestamp, body))
        .body(body.to_string())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("could not reach it: {}", e.without_url()))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("it answered {}", response.status().as_u16()))
    }
}

/// Posts the deliveries that are due.
async fn deliver(instance: &Instance) -> Result<(), String> {
    let s = instance.store.as_ref();
    let now = now_ms();
    let due = s
        .query(
            "SELECT d.id, d.event, d.body, d.attempts, h.id, h.url, h.secret FROM core_webhook_deliveries d JOIN core_webhooks h ON h.id = d.hook WHERE d.state = 'queued' AND d.next_at <= $1 ORDER BY d.next_at LIMIT 200",
            &[now.into()],
        )
        .await
        .map_err(|e| e.to_string())?;
    // At most 10 posts a hook a run: a slow or silent one does not hold
    // up the others for long.
    let mut per_hook: BTreeMap<String, usize> = BTreeMap::new();
    for r in &due {
        let hook_id = r.text(4).unwrap_or_default().to_string();
        let n = per_hook.entry(hook_id).or_default();
        if *n >= 10 {
            continue;
        }
        *n += 1;
        let (id, event, body, attempts, hook, url, sealed) = (
            r.text(0).unwrap_or_default(),
            r.text(1).unwrap_or_default(),
            r.text(2).unwrap_or_default(),
            r.int(3).unwrap_or(0),
            r.text(4).unwrap_or_default(),
            r.text(5).unwrap_or_default(),
            r.text(6).unwrap_or_default(),
        );
        let Ok(secret) = instance.keys.open_text(sealed) else {
            // A secret this installation cannot open (a key gone): that
            // hook fails, the others go on.
            let _ = s
                .execute(
                    "UPDATE core_webhooks SET last_error = $1 WHERE id = $2",
                    &["its secret cannot be opened".into(), hook.into()],
                )
                .await;
            continue;
        };
        crate::outbound::note(s, "webhook", url).await;
        let result = post(url, &secret, id, event, body).await;
        let tries = attempts + 1;
        match result {
            Ok(()) => {
                let _ = s.execute("UPDATE core_webhook_deliveries SET state = 'done', attempts = $1, last_error = NULL WHERE id = $2", &[tries.into(), id.into()]).await;
                let _ = s
                    .execute(
                        "UPDATE core_webhooks SET last_ok_at = $1, last_error = NULL WHERE id = $2",
                        &[now_ms().into(), hook.into()],
                    )
                    .await;
            }
            Err(e) => {
                // 2, 4, 8 … minutes apart; given up after ATTEMPTS.
                let state = if tries >= ATTEMPTS {
                    "failed"
                } else {
                    "queued"
                };
                let next = now_ms() + 60_000 * (1i64 << tries.min(10));
                let _ = s.execute("UPDATE core_webhook_deliveries SET state = $1, attempts = $2, next_at = $3, last_error = $4 WHERE id = $5", &[state.into(), tries.into(), next.into(), e.as_str().into(), id.into()]).await;
                let _ = s
                    .execute(
                        "UPDATE core_webhooks SET last_error = $1 WHERE id = $2",
                        &[e.as_str().into(), hook.into()],
                    )
                    .await;
                if state == "failed" {
                    let _ = crate::audit::record(
                        s,
                        &crate::audit::Event::new(
                            "webhooks.gave_up",
                            crate::audit::Outcome::Failed,
                        )
                        .on(hook)
                        .with(json!({"delivery": id, "event": event, "error": e})),
                        now_ms(),
                    )
                    .await;
                }
            }
        }
    }
    // Done deliveries are kept a week, failed ones a month.
    let _ = s
        .execute(
            "DELETE FROM core_webhook_deliveries WHERE (state = 'done' AND at < $1) OR (state = 'failed' AND at < $2)",
            &[(now - 7 * 86_400_000).into(), (now - 30 * 86_400_000).into()],
        )
        .await;
    Ok(())
}

/// One run: new events handed out, then the due deliveries posted.
pub async fn run(instance: &Instance) -> Result<(), String> {
    hand_out(instance).await?;
    deliver(instance).await
}

/// The job that runs it.
pub fn job(instance: Arc<Instance>) -> crate::app::OutsideJobKind {
    crate::app::OutsideJobKind {
        kind: JOB.into(),
        handler: Box::new(move |_| {
            let instance = Arc::clone(&instance);
            Box::pin(async move { run(&instance).await })
        }),
    }
}

/// The hooks, without their secrets.
pub async fn list(store: &dyn AsyncStore) -> Result<Vec<Value>, crate::storage::StoreError> {
    Ok(store
        .query(
            "SELECT h.id, h.name, h.url, h.events, h.created_at, h.last_ok_at, h.last_error, (SELECT COUNT(*) FROM core_webhook_deliveries d WHERE d.hook = h.id AND d.state = 'queued') FROM core_webhooks h ORDER BY h.created_at",
            &[],
        )
        .await?
        .iter()
        .map(|r| {
            json!({
                "id": r.text(0), "name": r.text(1), "url": r.text(2),
                "events": serde_json::from_str::<Value>(r.text(3).unwrap_or("[]")).unwrap_or(Value::Null),
                "createdAt": r.int(4), "lastOkAt": r.int(5), "lastError": r.text(6), "waiting": r.int(7),
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct NewHook {
    name: String,
    url: String,
    events: Vec<String>,
}

fn url_ok(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host = crate::outbound::host_of(url);
    url.len() <= 500
        && !host.is_empty()
        && !rest.contains(char::is_whitespace)
        && !url.chars().any(char::is_control)
        && !rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .contains('@')
}

async fn add(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let input: NewHook = body(request)?;
    let name: String = input
        .name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    let url = input.url.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("A webhook has a name."));
    }
    if !url_ok(&url) {
        return Err(ApiError::bad_request(
            "The address is an http or https address.",
        ));
    }
    let mut events: Vec<String> = input
        .events
        .into_iter()
        .filter(|e| EVENTS.iter().any(|(n, _)| n == e))
        .collect();
    events.sort();
    events.dedup();
    if events.is_empty() {
        return Err(ApiError::bad_request("Choose the events it hears of."));
    }
    let s = instance.store.as_ref();
    let count = s
        .query("SELECT COUNT(*) FROM core_webhooks", &[])
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    if count >= MAX_HOOKS {
        return Err(ApiError::conflict(
            "There are as many webhooks as an installation keeps.",
        ));
    }
    let from = s
        .query("SELECT COALESCE(MAX(seq), 0) FROM core_audit", &[])
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    let id = format!("w{}", crate::auth::random_hex(8));
    let secret = crate::auth::random_hex(24);
    s.execute(
        "INSERT INTO core_webhooks (id, name, url, events, secret, created_by, created_at, from_seq) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[id.as_str().into(), name.as_str().into(), url.as_str().into(), json!(events).to_string().into(), instance.keys.seal_text(&secret).into(), me.id.as_str().into(), now_ms().into(), from.into()],
    )
    .await?;
    crate::auth::audit(
        instance,
        crate::audit::Event::new("webhooks.added", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&id)
            .with(json!({"name": name, "host": crate::outbound::host_of(&url), "events": events})),
    )
    .await;
    // The secret is shown once.
    Ok(ok(
        &json!({"id": id, "secret": secret, "webhooks": list(s).await?}),
    ))
}

async fn remove(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let id = params.get("hook").unwrap_or_default();
    let s = instance.store.as_ref();
    let mut tx = s.begin().await?;
    tx.execute(
        "DELETE FROM core_webhook_deliveries WHERE hook = $1",
        &[id.into()],
    )
    .await?;
    let n = tx
        .execute("DELETE FROM core_webhooks WHERE id = $1", &[id.into()])
        .await?;
    tx.commit().await?;
    if n == 0 {
        return Err(ApiError::not_found("There is no such webhook."));
    }
    crate::auth::audit(
        instance,
        crate::audit::Event::new("webhooks.removed", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(id),
    )
    .await;
    Ok(ok(&json!({"webhooks": list(s).await?})))
}

/// A test post now ("ping"), its outcome at once.
async fn ping(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("hook").unwrap_or_default();
    let s = instance.store.as_ref();
    let rows = s
        .query(
            "SELECT url, secret FROM core_webhooks WHERE id = $1",
            &[id.into()],
        )
        .await?;
    let Some(r) = rows.first() else {
        return Err(ApiError::not_found("There is no such webhook."));
    };
    let url = r.text(0).unwrap_or_default().to_string();
    let secret = instance
        .keys
        .open_text(r.text(1).unwrap_or_default())
        .map_err(|_| ApiError::internal())?;
    let delivery = format!("p{}", crate::auth::random_hex(8));
    let body = json!({"event": "ping", "at": now_ms()}).to_string();
    crate::outbound::note(s, "webhook", &url).await;
    let result = post(&url, &secret, &delivery, "ping", &body).await;
    match &result {
        Ok(()) => {
            s.execute(
                "UPDATE core_webhooks SET last_ok_at = $1, last_error = NULL WHERE id = $2",
                &[now_ms().into(), id.into()],
            )
            .await?
        }
        Err(e) => {
            s.execute(
                "UPDATE core_webhooks SET last_error = $1 WHERE id = $2",
                &[e.as_str().into(), id.into()],
            )
            .await?
        }
    };
    Ok(ok(
        &json!({"ok": result.is_ok(), "error": result.err(), "webhooks": list(s).await?}),
    ))
}

/// The Integrations panel: webhooks, the directory, outside sign-in, and
/// what is not set up.
async fn integrations(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let s = instance.store.as_ref();
    let directory = s
        .query("SELECT url, last_sync_at, last_result FROM core_directory WHERE id = 1", &[])
        .await?
        .first()
        .map(|r| json!({"host": crate::outbound::host_of(r.text(0).unwrap_or_default()), "lastSyncAt": r.int(1), "lastResult": r.text(2)}));
    let providers: Vec<Value> = s
        .query("SELECT name, issuer FROM core_external_providers ORDER BY created_at", &[])
        .await?
        .iter()
        .map(|r| json!({"name": r.text(0), "host": crate::outbound::host_of(r.text(1).unwrap_or_default())}))
        .collect();
    let events: Vec<&str> = EVENTS.iter().map(|(n, _)| *n).collect();
    Ok(ok(
        &json!({"webhooks": list(s).await?, "events": events, "directory": directory, "signIn": providers}),
    ))
}

fn h_integrations(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, integrations(&request, &instance).await) })
}

fn h_add(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, add(&request, &instance).await) })
}

fn h_remove(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, remove(&request, &params, &instance).await) })
}

fn h_ping(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, ping(&request, &params, &instance).await) })
}

pub fn routes() -> Vec<Route> {
    let object = || json!({"type": "object"});
    vec![
        Route { method: Method::Get, path: "/admin/integrations", id: "integrations", tag: "admin", summary: "What Ortus is connected to: outgoing webhooks (with their last delivery), the directory sync, outside sign-in providers; and the events a webhook can hear of.", access: Access::Admin, response: object, handler: h_integrations },
        Route { method: Method::Post, path: "/admin/webhooks", id: "webhookAdd", tag: "admin", summary: "Adds an outgoing webhook: {name, url, events}; gives its signing secret once. Events: record.filed, approval.done, document.signed, task.created, person.invited, person.joined, person.removed, backup.restored.", access: Access::Owner, response: object, handler: h_add },
        Route { method: Method::Delete, path: "/admin/webhooks/:hook", id: "webhookRemove", tag: "admin", summary: "Removes an outgoing webhook and what it had still to send.", access: Access::Owner, response: object, handler: h_remove },
        Route { method: Method::Post, path: "/admin/webhooks/:hook/ping", id: "webhookPing", tag: "admin", summary: "Sends the webhook a test post now; says whether it was taken.", access: Access::Admin, response: object, handler: h_ping },
    ]
}

/// The API reference of this installation (the core's operations and its
/// apps'), as OpenAPI: what an integrator builds against.
async fn reference(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let mut all = crate::api::routes();
    for app in &instance.apps {
        all.extend(app.routes());
    }
    Ok(ok(&crate::api::reference(&all)))
}

fn h_reference(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, reference(&request, &instance).await) })
}

/// The reference route (signed in).
pub fn reference_route() -> Route {
    Route {
        method: Method::Get,
        path: "/reference",
        id: "reference",
        tag: "meta",
        summary:
            "This installation's API reference (OpenAPI 3.1): the core's operations and its apps'.",
        access: Access::Signed,
        response: || json!({"type": "object"}),
        handler: h_reference,
    }
}
