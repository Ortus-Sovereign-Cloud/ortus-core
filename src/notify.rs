//! Notifications: what apps tell a person (a mention, an approval waiting
//! for them, an alert, an update), shown by the bell on the rail and in
//! the notification centre, newest first. Each person sets quiet hours and
//! do not disturb (phones stay silent then; the centre still lists them).
//!
//! Apps add them with [`add`] (or [`add_in`] inside their transaction).

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::auth::Access;
use crate::instance::Instance;
use crate::storage::{AsyncStore, AsyncTx, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "notifications and quiet hours",
    sql: "CREATE TABLE core_notifications (
        id TEXT PRIMARY KEY,
        user_id TEXT NOT NULL,
        app TEXT NOT NULL,
        kind TEXT NOT NULL,
        title TEXT NOT NULL,
        detail TEXT NOT NULL,
        path TEXT NOT NULL,
        template TEXT,
        at BIGINT NOT NULL,
        read_at BIGINT
    );
    CREATE INDEX core_notifications_user ON core_notifications (user_id, at);
    CREATE TABLE core_notify_settings (
        user_id TEXT PRIMARY KEY,
        quiet_from TEXT,
        quiet_to TEXT,
        dnd BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.notify",
    migrations: MIGRATIONS,
};

/// What a notification is about (the centre's tabs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Someone mentioned the person.
    Mention,
    /// Something waits for the person's approval or signature.
    Approval,
    /// An alert from a monitored source (a gauge, a system).
    Alert,
    /// Anything else worth knowing (a task accepted, a room booked).
    Update,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Mention => "mention",
            Kind::Approval => "approval",
            Kind::Alert => "alert",
            Kind::Update => "update",
        }
    }
}

/// A notification an app adds.
pub struct Notice<'a> {
    /// The app it comes from ("chat", "docs").
    pub app: &'a str,
    pub kind: Kind,
    /// One line: "Anna mentioned you in #flood-response".
    pub title: &'a str,
    /// A short second line ("due 14:00"), or empty.
    pub detail: &'a str,
    /// Where it opens in the web client ("/chat?room=…").
    pub path: &'a str,
    /// The title as interface text, shown in each person's language: a
    /// template of the core's ("mention": "{who} mentioned you in
    /// {where}"; also review, task, taskDone, suggestion, record, invite, mailDelayed,
    /// mailNotDelivered)
    /// with its `args`; `title` is the fallback. None for content (an
    /// alert's own words).
    pub key: Option<&'a str>,
    /// The template's values; a `due` (Unix ms) also shows "due 14:00".
    pub args: serde_json::Value,
}

/// Notifications kept per person (older ones go).
const KEEP: i64 = 500;

pub async fn add(store: &dyn AsyncStore, user: &str, n: &Notice<'_>) -> StoreResult<()> {
    let mut tx = store.begin().await?;
    add_in(tx.as_mut(), user, n).await?;
    tx.commit().await
}

/// Adds a notification inside the caller's transaction.
pub async fn add_in(tx: &mut dyn AsyncTx, user: &str, n: &Notice<'_>) -> StoreResult<()> {
    let clip = |s: &str, max: usize| -> String {
        s.chars().filter(|c| !c.is_control()).take(max).collect()
    };
    // Paths stay inside Ortus (checked after cleaning; no "//" or "/\").
    let cleaned = clip(n.path, 500);
    let path = if cleaned.starts_with('/') && !cleaned.starts_with("//") && !cleaned.contains('\\')
    {
        cleaned
    } else {
        "/".to_string()
    };
    let template = json!({"key": n.key, "args": n.args}).to_string();
    tx.execute(
        "INSERT INTO core_notifications (id, user_id, app, kind, title, detail, path, template, at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &[
            format!("n{}", crate::auth::random_hex(12)).into(),
            user.into(),
            clip(n.app, 40).into(),
            n.kind.name().into(),
            clip(n.title, 300).into(),
            clip(n.detail, 200).into(),
            path.into(),
            clip(&template, 2000).into(),
            crate::program::now_ms().into(),
        ],
    )
    .await?;
    // Beyond the newest KEEP of each sort, the oldest go: approvals are
    // kept apart, so a noisy feed of alerts never pushes them out (a form
    // both databases take).
    let approval = n.kind == Kind::Approval;
    let same = if approval {
        "kind = 'approval'"
    } else {
        "kind <> 'approval'"
    };
    let cut = tx
        .query(&format!("SELECT at FROM core_notifications WHERE user_id = $1 AND {same} ORDER BY at DESC LIMIT 1 OFFSET $2"), &[user.into(), KEEP.into()])
        .await?
        .first()
        .and_then(|r| r.int(0));
    if let Some(at) = cut {
        tx.execute(
            &format!("DELETE FROM core_notifications WHERE user_id = $1 AND {same} AND at <= $2"),
            &[user.into(), at.into()],
        )
        .await?;
    }
    Ok(())
}

async fn list(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let s = instance.store.as_ref();
    let rows = s
        .query("SELECT id, app, kind, title, detail, path, at, read_at, template FROM core_notifications WHERE user_id = $1 ORDER BY at DESC, id DESC LIMIT 50", &[me.id.as_str().into()])
        .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let template: Value = serde_json::from_str(r.text(8).unwrap_or("null")).unwrap_or(Value::Null);
            json!({"id": r.text(0), "app": r.text(1), "kind": r.text(2), "title": r.text(3), "detail": r.text(4), "path": r.text(5), "at": r.int(6), "read": r.int(7).is_some(), "key": template["key"], "args": template["args"]})
        })
        .collect();
    let mut counts = json!({"mention": 0, "approval": 0, "alert": 0, "update": 0});
    for r in s.query("SELECT kind, COUNT(*) FROM core_notifications WHERE user_id = $1 AND read_at IS NULL GROUP BY kind", &[me.id.as_str().into()]).await? {
        if let Some(k) = r.text(0) {
            counts[k] = json!(r.int(1).unwrap_or(0));
        }
    }
    let unread: i64 = ["mention", "approval", "alert", "update"]
        .iter()
        .map(|k| counts[*k].as_i64().unwrap_or(0))
        .sum();
    let settings = s
        .query(
            "SELECT quiet_from, quiet_to, dnd FROM core_notify_settings WHERE user_id = $1",
            &[me.id.as_str().into()],
        )
        .await?;
    let quiet = settings
        .first()
        .map(|r| json!({"from": r.text(0), "to": r.text(1), "dnd": r.int(2).unwrap_or(0) != 0}))
        .unwrap_or(json!({"from": null, "to": null, "dnd": false}));
    Ok(ok(
        &json!({"items": items, "unread": unread, "counts": counts, "quiet": quiet}),
    ))
}

#[derive(Deserialize)]
struct Read {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    all: bool,
}

async fn read(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let input: Read = body(request)?;
    let s = instance.store.as_ref();
    let now = crate::program::now_ms();
    if input.all {
        s.execute(
            "UPDATE core_notifications SET read_at = $1 WHERE user_id = $2 AND read_at IS NULL",
            &[now.into(), me.id.as_str().into()],
        )
        .await?;
    }
    for id in input.ids.iter().take(200) {
        s.execute("UPDATE core_notifications SET read_at = $1 WHERE id = $2 AND user_id = $3 AND read_at IS NULL", &[now.into(), id.as_str().into(), me.id.as_str().into()]).await?;
    }
    list(request, instance).await
}

/// A field left out keeps its value; `null` clears it.
fn field<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct Settings {
    /// Quiet hours, "08:00" style, both or neither.
    #[serde(rename = "quietFrom", default, deserialize_with = "field")]
    quiet_from: Option<Option<String>>,
    #[serde(rename = "quietTo", default, deserialize_with = "field")]
    quiet_to: Option<Option<String>>,
    dnd: Option<bool>,
}

fn hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && s[..2].parse::<u8>().is_ok_and(|h| h < 24)
        && s[3..].parse::<u8>().is_ok_and(|m| m < 60)
}

async fn settings(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let input: Settings = body(request)?;
    let s = instance.store.as_ref();
    let had = s
        .query(
            "SELECT quiet_from, quiet_to, dnd FROM core_notify_settings WHERE user_id = $1",
            &[me.id.as_str().into()],
        )
        .await?;
    let stored = |i: usize| had.first().and_then(|r| r.text(i).map(str::to_string));
    let quiet_from = input.quiet_from.unwrap_or_else(|| stored(0));
    let quiet_to = input.quiet_to.unwrap_or_else(|| stored(1));
    if quiet_from.as_deref().is_some_and(|v| !hhmm(v))
        || quiet_to.as_deref().is_some_and(|v| !hhmm(v))
        || quiet_from.is_some() != quiet_to.is_some()
    {
        return Err(ApiError::bad_request(
            "Quiet hours are a start and an end, like 17:00 and 08:00.",
        ));
    }
    let dnd = input
        .dnd
        .unwrap_or_else(|| had.first().and_then(|r| r.int(2)).unwrap_or(0) != 0);
    // One statement (two tabs at once do not collide on the key).
    s.execute(
        "INSERT INTO core_notify_settings (user_id, quiet_from, quiet_to, dnd) VALUES ($1, $2, $3, $4) ON CONFLICT (user_id) DO UPDATE SET quiet_from = excluded.quiet_from, quiet_to = excluded.quiet_to, dnd = excluded.dnd",
        &[me.id.as_str().into(), quiet_from.into(), quiet_to.into(), i64::from(dnd).into()],
    )
    .await?;
    list(request, instance).await
}

/// Whether a phone should stay silent for this person now (do not
/// disturb, or within quiet hours at `minute_of_day` in their time).
pub async fn quiet(store: &dyn AsyncStore, user: &str, minute_of_day: u32) -> StoreResult<bool> {
    let rows = store
        .query(
            "SELECT quiet_from, quiet_to, dnd FROM core_notify_settings WHERE user_id = $1",
            &[user.into()],
        )
        .await?;
    let Some(r) = rows.first() else {
        return Ok(false);
    };
    if r.int(2).unwrap_or(0) != 0 {
        return Ok(true);
    }
    let minute = |s: Option<&str>| {
        s.filter(|v| hhmm(v))
            .map(|v| v[..2].parse::<u32>().unwrap_or(0) * 60 + v[3..].parse::<u32>().unwrap_or(0))
    };
    Ok(match (minute(r.text(0)), minute(r.text(1))) {
        (Some(from), Some(to)) if from <= to => (from..to).contains(&minute_of_day),
        (Some(from), Some(to)) => minute_of_day >= from || minute_of_day < to,
        _ => false,
    })
}

fn h_list(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, list(&request, &instance).await) })
}

fn h_read(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, read(&request, &instance).await) })
}

fn h_settings(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, settings(&request, &instance).await) })
}

pub fn routes() -> Vec<Route> {
    let object = || json!({"type": "object"});
    vec![
        Route { method: Method::Get, path: "/notifications", id: "notifications", tag: "notifications", summary: "One's notifications, newest first, with unread counts by kind (mention, approval, alert, update) and quiet hours.", access: Access::Signed, response: object, handler: h_list },
        Route { method: Method::Post, path: "/notifications/read", id: "notificationsRead", tag: "notifications", summary: "Marks notifications read: {ids} or {all: true}.", access: Access::Signed, response: object, handler: h_read },
        Route { method: Method::Patch, path: "/notifications/settings", id: "notificationsSettings", tag: "notifications", summary: "Quiet hours and do not disturb: {quietFrom, quietTo, dnd}.", access: Access::Signed, response: object, handler: h_settings },
    ]
}
