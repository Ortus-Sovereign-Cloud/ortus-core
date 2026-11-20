//! How long things are kept, and legal holds that keep them longer.
//!
//! The owner sets a period per kind (mail, chat, call recordings, files,
//! things people deleted). Once a day the core sweep hands every app the
//! [`Rules`] in one transaction (`App::purge`); the app removes what is past
//! its period and not on hold. A legal hold (a case reference, people and
//! channels) stops all of that, and stops expired accounts being deleted.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::app::JobKind;
use crate::auth::{accounts, Access};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, AsyncTx, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "retention and holds",
    sql: "CREATE TABLE core_retention (
        kind TEXT PRIMARY KEY,
        days BIGINT,
        changed_at BIGINT NOT NULL
    );
    INSERT INTO core_retention (kind, days, changed_at) VALUES
        ('mail', 3650, 0), ('chat', 2555, 0), ('recordings', 365, 0), ('files', NULL, 0), ('deleted', 90, 0);
    CREATE TABLE core_holds (
        id TEXT PRIMARY KEY,
        case_ref TEXT NOT NULL,
        note TEXT NOT NULL,
        created_by TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        released_at BIGINT
    );
    CREATE TABLE core_hold_targets (
        hold_id TEXT NOT NULL REFERENCES core_holds (id),
        scope TEXT NOT NULL,
        target TEXT NOT NULL,
        PRIMARY KEY (hold_id, scope, target)
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.retention",
    migrations: MIGRATIONS,
};

/// The kinds a period is set for.
pub const KINDS: [&str; 5] = ["mail", "chat", "recordings", "files", "deleted"];

/// The sweep's job kind (daily).
pub const SWEEP_JOB: &str = "core.retention-sweep";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Everything an app needs to decide what may go, read once per sweep.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// Days per kind; absent or None means kept for ever.
    pub days: HashMap<String, Option<i64>>,
    /// People (user ids) on an active hold.
    pub held_people: HashSet<String>,
    /// Channels (an app's own ids, such as `chat:room:42`) on an active hold.
    pub held_channels: HashSet<String>,
    pub now: i64,
}

impl Rules {
    /// Whether something of `kind` made at `created_at` is past its period.
    pub fn expired(&self, kind: &str, created_at: i64) -> bool {
        match self.days.get(kind).copied().flatten() {
            Some(days) => created_at + days * DAY_MS <= self.now,
            None => false,
        }
    }

    /// Whether it may be removed: past its period and on no hold through
    /// any of its people or its channel.
    pub fn may_purge(
        &self,
        kind: &str,
        created_at: i64,
        people: &[&str],
        channel: Option<&str>,
    ) -> bool {
        self.expired(kind, created_at)
            && !people.iter().any(|p| self.held_people.contains(*p))
            && !channel.is_some_and(|c| self.held_channels.contains(c))
    }
}

async fn read_rules(tx: &mut dyn AsyncTx, now: i64) -> StoreResult<Rules> {
    let mut rules = Rules {
        now,
        ..Rules::default()
    };
    for r in tx
        .query("SELECT kind, days FROM core_retention", &[])
        .await?
    {
        rules
            .days
            .insert(r.text(0).unwrap_or_default().to_string(), r.int(1));
    }
    for r in tx
        .query("SELECT t.scope, t.target FROM core_hold_targets t JOIN core_holds h ON h.id = t.hold_id WHERE h.released_at IS NULL", &[])
        .await?
    {
        let target = r.text(1).unwrap_or_default().to_string();
        match r.text(0) {
            Some("person") => rules.held_people.insert(target),
            _ => rules.held_channels.insert(target),
        };
    }
    Ok(rules)
}

/// The rules now, outside a sweep (for screens and single checks).
pub async fn rules(store: &dyn AsyncStore) -> StoreResult<Rules> {
    let mut tx = store.begin_read().await?;
    let rules = read_rules(&mut *tx, now_ms()).await;
    tx.rollback().await?;
    rules
}

/// Whether a person is on an active hold.
pub async fn person_on_hold(store: &dyn AsyncStore, user_id: &str) -> StoreResult<bool> {
    Ok(rules(store).await?.held_people.contains(user_id))
}

/// SQL that is true for a user id on an active hold (for the account sweep).
pub const PERSON_HELD_SQL: &str = "SELECT t.target FROM core_hold_targets t JOIN core_holds h ON h.id = t.hold_id WHERE h.released_at IS NULL AND t.scope = 'person'";

/// The daily sweep: every app purges by the same rules in one transaction.
pub fn sweep_job(ortus: Arc<Instance>) -> JobKind {
    JobKind {
        kind: SWEEP_JOB.into(),
        handler: Box::new(move |_job, tx| {
            let ortus = Arc::clone(&ortus);
            Box::pin(async move {
                let now = now_ms();
                let rules = read_rules(tx, now).await.map_err(|e| e.to_string())?;
                let mut removed = serde_json::Map::new();
                for app in &ortus.apps {
                    let n = app.purge(tx, &rules).await?;
                    if n > 0 {
                        removed.insert(app.info().id.to_string(), n.into());
                    }
                }
                if !removed.is_empty() {
                    crate::audit::record_in(
                        tx,
                        &crate::audit::Event::new("retention.purged", crate::audit::Outcome::Done)
                            .with(Value::Object(removed)),
                        now,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Ok(())
            })
        }),
    }
}

// The admin API.

async fn show(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let store = ortus.store.as_ref();
    let periods: Vec<Value> = store
        .query("SELECT kind, days FROM core_retention", &[])
        .await?
        .iter()
        .map(|r| json!({ "kind": r.text(0), "days": r.int(1) }))
        .collect();
    let mut periods = periods;
    periods.sort_by_key(|p| {
        KINDS
            .iter()
            .position(|k| Some(*k) == p["kind"].as_str())
            .unwrap_or(99)
    });
    let mut holds = Vec::new();
    for h in store.query("SELECT id, case_ref, note, created_at, released_at FROM core_holds ORDER BY created_at DESC", &[]).await? {
        let id = h.text(0).unwrap_or_default().to_string();
        let targets = store.query("SELECT scope, target FROM core_hold_targets WHERE hold_id = $1 ORDER BY scope, target", &[id.as_str().into()]).await?;
        let mut people = Vec::new();
        let mut channels = Vec::new();
        for t in &targets {
            let target = t.text(1).unwrap_or_default();
            if t.text(0) == Some("person") {
                let user = accounts::by_id(store, target).await?;
                people.push(json!({ "id": target, "email": user.as_ref().map(|u| u.email.clone()), "name": user.map(|u| u.name) }));
            } else {
                channels.push(json!(target));
            }
        }
        holds.push(json!({ "id": id, "case_ref": h.text(1), "note": h.text(2), "created_at": h.int(3), "released_at": h.int(4), "people": people, "channels": channels }));
    }
    Ok(ok(&json!({ "periods": periods, "holds": holds })))
}

#[derive(Deserialize)]
struct Period {
    /// None keeps for ever.
    days: Option<i64>,
}

async fn set_period(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let kind = params.get("kind").unwrap_or_default().to_string();
    if !KINDS.contains(&kind.as_str()) {
        return Err(ApiError::not_found("There is no such kind."));
    }
    let input: Period = body(request)?;
    if input.days.is_some_and(|d| !(1..=36_500).contains(&d)) {
        return Err(ApiError::bad_request(
            "Keep things 1 to 36500 days, or for ever (null).",
        ));
    }
    ortus
        .store
        .execute(
            "UPDATE core_retention SET days = $1, changed_at = $2 WHERE kind = $3",
            &[input.days.into(), now_ms().into(), kind.as_str().into()],
        )
        .await?;
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("retention.changed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "kind": kind, "days": input.days })),
    )
    .await;
    show(request, ortus).await
}

#[derive(Deserialize)]
struct NewHold {
    case_ref: String,
    #[serde(default)]
    note: String,
    /// Addresses of people here.
    #[serde(default)]
    people: Vec<String>,
    /// Channels, by an app's own id.
    #[serde(default)]
    channels: Vec<String>,
}

async fn add_hold(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: NewHold = body(request)?;
    let case_ref = input.case_ref.trim();
    if case_ref.is_empty()
        || case_ref.chars().count() > 100
        || (input.people.is_empty() && input.channels.is_empty())
    {
        return Err(ApiError::bad_request(
            "A hold has a case reference and at least one person or channel.",
        ));
    }
    let store = ortus.store.as_ref();
    let mut people = Vec::new();
    for email in &input.people {
        let user = accounts::by_email(store, email).await?.ok_or_else(|| {
            ApiError::bad_request(format!("Nobody here has the address {email}."))
        })?;
        people.push(user.id);
    }
    let id = format!("hold_{}", crate::auth::random_hex(12));
    let mut tx = store.begin().await?;
    tx.execute(
        "INSERT INTO core_holds (id, case_ref, note, created_by, created_at) VALUES ($1, $2, $3, $4, $5)",
        &[id.as_str().into(), case_ref.into(), input.note.trim().into(), me.user.id.as_str().into(), now_ms().into()],
    )
    .await?;
    let channels: HashSet<&str> = input
        .channels
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect();
    let people: HashSet<&str> = people.iter().map(String::as_str).collect();
    for (scope, targets) in [("person", &people), ("channel", &channels)] {
        for target in targets {
            tx.execute(
                "INSERT INTO core_hold_targets (hold_id, scope, target) VALUES ($1, $2, $3)",
                &[id.as_str().into(), scope.into(), (*target).into()],
            )
            .await?;
        }
    }
    tx.commit().await?;
    crate::auth::audit(ortus, crate::audit::Event::new("retention.hold_placed", crate::audit::Outcome::Done).by(&me.user.id).with(json!({ "hold": id, "case_ref": case_ref, "people": people.len(), "channels": channels.len() }))).await;
    show(request, ortus).await
}

async fn release_hold(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default().to_string();
    let released = ortus
        .store
        .execute(
            "UPDATE core_holds SET released_at = $1 WHERE id = $2 AND released_at IS NULL",
            &[now_ms().into(), id.as_str().into()],
        )
        .await?;
    if released == 0 {
        return Err(ApiError::not_found("There is no such active hold."));
    }
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("retention.hold_released", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({ "hold": id })),
    )
    .await;
    show(request, ortus).await
}

macro_rules! h {
    ($name:ident, $inner:ident) => {
        fn $name(
            request: Request,
            _: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &ortus).await) })
        }
    };
    ($name:ident, $inner:ident, params) => {
        fn $name(
            request: Request,
            params: PathParams,
            _: ConnectionInfo,
            ortus: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $inner(&request, &params, &ortus).await) })
        }
    };
}

h!(h_show, show);
h!(h_set_period, set_period, params);
h!(h_add_hold, add_hold);
h!(h_release_hold, release_hold, params);

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, access, handler| Route {
        method,
        path,
        id,
        tag: "retention",
        summary,
        access,
        response: any_object,
        handler,
    };
    vec![
        r(Method::Get, "/admin/retention", "getRetention", "How long each kind is kept (days, or null for ever) and the legal holds.", Access::Admin, h_show as crate::api::Handler),
        r(Method::Put, "/admin/retention/:kind", "setRetention", "Set how long a kind (mail, chat, recordings, files, deleted) is kept: {days} (null for ever).", Access::Owner, h_set_period),
        r(Method::Post, "/admin/holds", "placeHold", "Place a legal hold: {case_ref, note, people (addresses), channels (app ids)}. Nothing they hold is removed until it is released.", Access::Owner, h_add_hold),
        r(Method::Post, "/admin/holds/:id/release", "releaseHold", "Release a legal hold.", Access::Owner, h_release_hold),
    ]
}
