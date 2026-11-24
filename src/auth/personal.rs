//! One's own settings and first day: language, text size and motion, one's
//! name, one's signed-in devices, and the welcome page's three steps and
//! people.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::accounts::{self, Field};
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "preferences",
    sql: "CREATE TABLE core_preferences (
        user_id TEXT PRIMARY KEY REFERENCES core_users (id),
        value TEXT NOT NULL,
        changed_at BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.personal",
    migrations: MIGRATIONS,
};

/// The text sizes offered, in percent.
const TEXT_SIZES: [i64; 4] = [100, 110, 125, 150];
/// The languages the interface has.
const LANGUAGES: [&str; 8] = ["en", "pl", "uk", "de", "fr", "es", "it", "ar"];

async fn preferences(store: &dyn AsyncStore, user_id: &str) -> StoreResult<Map<String, Value>> {
    let rows = store
        .query(
            "SELECT value FROM core_preferences WHERE user_id = $1",
            &[user_id.into()],
        )
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.text(0))
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default())
}

async fn save(
    store: &dyn AsyncStore,
    user_id: &str,
    value: &Map<String, Value>,
) -> StoreResult<()> {
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_preferences WHERE user_id = $1",
        &[user_id.into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_preferences (user_id, value, changed_at) VALUES ($1, $2, $3)",
        &[
            user_id.into(),
            Value::Object(value.clone()).to_string().into(),
            now_ms().into(),
        ],
    )
    .await?;
    tx.commit().await
}

fn shown(language: Option<&str>, prefs: &Map<String, Value>) -> Value {
    json!({
        "language": language,
        "text_size": prefs.get("text_size").and_then(Value::as_i64).unwrap_or(100),
        "motion": prefs.get("motion").and_then(Value::as_str).unwrap_or("system"),
    })
}

async fn get_preferences(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let prefs = preferences(ortus.store.as_ref(), &me.user.id).await?;
    Ok(ok(&shown(me.user.language.as_deref(), &prefs)))
}

#[derive(Deserialize)]
struct PreferencesChange {
    /// Absent: unchanged; null: follow the browser.
    #[serde(default, deserialize_with = "present")]
    language: Option<Option<String>>,
    text_size: Option<i64>,
    motion: Option<String>,
}

/// A field that is there, even as null, is `Some`.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

async fn set_preferences(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: PreferencesChange = body(request)?;
    let store = ortus.store.as_ref();
    let mut prefs = preferences(store, &me.user.id).await?;
    let mut language = me.user.language.clone();
    if let Some(choice) = input.language {
        if let Some(code) = choice.as_deref() {
            if !LANGUAGES.contains(&code) {
                return Err(ApiError::bad_request("That language is not offered."));
            }
        }
        accounts::set(store, &me.user.id, Field::Language(choice.as_deref())).await?;
        language = choice;
    }
    if let Some(size) = input.text_size {
        if !TEXT_SIZES.contains(&size) {
            return Err(ApiError::bad_request(
                "Text size is 100, 110, 125 or 150 percent.",
            ));
        }
        prefs.insert("text_size".into(), size.into());
    }
    if let Some(motion) = input.motion {
        if !matches!(motion.as_str(), "system" | "reduce") {
            return Err(ApiError::bad_request("Motion is system or reduce."));
        }
        prefs.insert("motion".into(), motion.into());
    }
    save(store, &me.user.id, &prefs).await?;
    Ok(ok(&shown(language.as_deref(), &prefs)))
}

#[derive(Deserialize)]
struct Profile {
    name: String,
}

async fn set_profile(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: Profile = body(request)?;
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ApiError::bad_request("A name is 1 to 200 characters."));
    }
    accounts::set(ortus.store.as_ref(), &me.user.id, Field::Name(name)).await?;
    Ok(ok(&json!({ "name": name })))
}

async fn my_sessions(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let sessions = super::sessions::list(ortus.store.as_ref(), &me.user.id).await?;
    let list: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.token_hash, "current": s.token_hash == me.token_hash, "agent": s.agent, "address": s.address,
                "methods": s.methods, "created_at": s.created_at, "last_seen_at": s.last_seen_at,
            })
        })
        .collect();
    Ok(ok(&json!({ "sessions": list })))
}

async fn end_session(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default();
    let ended = ortus
        .store
        .execute(
            "DELETE FROM core_sessions WHERE token_hash = $1 AND user_id = $2",
            &[id.into(), me.user.id.as_str().into()],
        )
        .await?;
    if ended == 0 {
        return Err(ApiError::not_found("No such session."));
    }
    super::audit(
        ortus,
        crate::audit::Event::new("auth.session_ended", crate::audit::Outcome::Done).by(&me.user.id),
    )
    .await;
    Ok(ok(&json!({ "ended": true })))
}

/// The three first-day steps, and the people to meet: colleagues from
/// one's groups, and an administrator for help.
async fn welcome(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let store = ortus.store.as_ref();
    let secure = store
        .query("SELECT COUNT(*) FROM core_credentials WHERE user_id = $1 AND kind IN ('passkey', 'totp')", &[me.user.id.as_str().into()])
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0)
        > 0;
    let prefs = preferences(store, &me.user.id).await?;
    let groups: Vec<String> = store
        .query(
            "SELECT g.name FROM core_groups g JOIN core_group_members m ON m.group_id = g.id WHERE m.user_id = $1 ORDER BY g.name",
            &[me.user.id.as_str().into()],
        )
        .await?
        .iter()
        .filter_map(|r| r.text(0).map(str::to_string))
        .collect();
    let mut people: Vec<Value> = store
        .query(
            "SELECT DISTINCT u.name, u.email, g.name FROM core_group_members mine
             JOIN core_group_members theirs ON theirs.group_id = mine.group_id AND theirs.user_id <> mine.user_id
             JOIN core_users u ON u.id = theirs.user_id AND u.state = 'active'
             JOIN core_groups g ON g.id = mine.group_id
             WHERE mine.user_id = $1 ORDER BY u.name LIMIT 2",
            &[me.user.id.as_str().into()],
        )
        .await?
        .iter()
        .map(|r| json!({ "name": r.text(0), "email": r.text(1), "group": r.text(2) }))
        .collect();
    let help = store
        .query(
            "SELECT name, email FROM core_users WHERE role IN ('owner', 'admin') AND state = 'active' AND id <> $1 ORDER BY role DESC, name LIMIT 1",
            &[me.user.id.as_str().into()],
        )
        .await?;
    if let Some(r) = help.first() {
        people.push(json!({ "name": r.text(0), "email": r.text(1), "help": true }));
    }
    let steps = [
        ("secure", secure),
        ("policies", prefs.contains_key("policies_read")),
        ("team", prefs.contains_key("team_greeted")),
    ];
    Ok(ok(&json!({
        "name": me.user.name,
        "email": me.user.email,
        "steps": steps.iter().map(|(id, done)| json!({ "id": id, "done": done })).collect::<Vec<_>>(),
        "groups": groups,
        "people": people,
    })))
}

async fn welcome_step(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let key = match params.get("step").unwrap_or_default() {
        "policies" => "policies_read",
        "team" => "team_greeted",
        _ => return Err(ApiError::not_found("No such step.")),
    };
    let store = ortus.store.as_ref();
    let mut prefs = preferences(store, &me.user.id).await?;
    prefs.entry(key).or_insert_with(|| now_ms().into());
    save(store, &me.user.id, &prefs).await?;
    welcome(request, ortus).await
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

h!(h_get_preferences, get_preferences);
h!(h_set_preferences, set_preferences);
h!(h_set_profile, set_profile);
h!(h_sessions, my_sessions);
h!(h_end_session, end_session, params);
h!(h_welcome, welcome);
h!(h_welcome_step, welcome_step, params);

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, handler| Route {
        method,
        path,
        id,
        tag: "me",
        summary,
        access: Access::Signed,
        response: any_object,
        handler,
    };
    vec![
        r(Method::Get, "/me/preferences", "getMyPreferences", "One's language (null: the browser's), text size (percent) and motion (system or reduce).", h_get_preferences),
        r(Method::Put, "/me/preferences", "setMyPreferences", "Change any of {language, text_size, motion}.", h_set_preferences),
        r(Method::Put, "/me/profile", "setMyProfile", "Change one's name: {name}.", h_set_profile),
        r(Method::Get, "/me/sessions", "listMySessions", "The devices one is signed in on, the current one marked.", h_sessions),
        r(Method::Delete, "/me/sessions/:id", "endMySession", "Sign one device out.", h_end_session),
        r(Method::Get, "/me/welcome", "getMyWelcome", "The first-day steps (secure sign-in, read the policies, meet the team), one's groups and people to meet.", h_welcome),
        r(Method::Post, "/me/welcome/:step", "finishWelcomeStep", "Mark a first-day step done: policies or team.", h_welcome_step),
    ]
}
