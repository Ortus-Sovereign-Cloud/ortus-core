//! The search index every app feeds (mail, files, events, people) and the
//! one search route that reads it.
//!
//! The rows of `core_search` are the truth: an item's title, its text, a
//! little JSON (`meta`: where it opens, who it is from, ...) and whose it
//! is. Full-text lookup is the database's own: an FTS5 index on SQLite
//! (kept in step by [`index`] and [`remove`], left out of backups and
//! rebuilt on restore) and a GIN index on PostgreSQL.
//!
//! Results are only what the person may see: their own items, and items
//! of others that an app vouches for ([`crate::app::App::search_owners`],
//! [`crate::app::App::search_allowed`]).

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde_json::{json, Value};

use crate::api::{answer, ok, ApiError, Method, Route};
use crate::auth::Access;
use crate::instance::Instance;
use crate::storage::{AsyncStore, BoxFuture, Engine, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "search index",
    sql: "CREATE TABLE core_search (
        app TEXT NOT NULL,
        id TEXT NOT NULL,
        owner TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        meta TEXT NOT NULL,
        at BIGINT NOT NULL,
        PRIMARY KEY (app, id)
    );
    CREATE INDEX core_search_owner ON core_search (owner, at);
    {{sqlite}} CREATE VIRTUAL TABLE core_search_fts USING fts5(title, body, content='core_search', tokenize='unicode61 remove_diacritics 2');
    {{postgres}} CREATE INDEX core_search_text ON core_search USING GIN (to_tsvector('simple', title || ' ' || body))",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.search",
    migrations: MIGRATIONS,
};

/// Text kept per item.
const MAX_BODY: usize = 100_000;

/// An item as an app hands it to the index.
pub struct Entry<'a> {
    pub app: &'a str,
    pub id: &'a str,
    /// The account whose item it is.
    pub owner: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    /// Shown with the result: `path` (where it opens) and anything the app
    /// wants (`from`, `attachment`, `start`, `size`, ...).
    pub meta: Value,
    /// When it happened (Unix ms): newest first among equals.
    pub at: i64,
}

fn clip(text: &str, chars: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(chars)
        .collect()
}

/// Adds or replaces an item in the index.
pub async fn index(store: &dyn AsyncStore, entry: &Entry<'_>) -> StoreResult<()> {
    let title = clip(entry.title, 500);
    let body = clip(entry.body, MAX_BODY);
    let mut tx = store.begin().await?;
    let sqlite = store.engine() == Engine::Sqlite;
    if sqlite {
        let old = tx
            .query(
                "SELECT rowid, title, body FROM core_search WHERE app = $1 AND id = $2",
                &[entry.app.into(), entry.id.into()],
            )
            .await?;
        if let Some(r) = old.first() {
            tx.execute(
                "INSERT INTO core_search_fts (core_search_fts, rowid, title, body) VALUES ('delete', $1, $2, $3)",
                &[r.int(0).unwrap_or(0).into(), r.text(1).unwrap_or_default().into(), r.text(2).unwrap_or_default().into()],
            )
            .await?;
        }
    }
    tx.execute(
        "DELETE FROM core_search WHERE app = $1 AND id = $2",
        &[entry.app.into(), entry.id.into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_search (app, id, owner, title, body, meta, at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        &[entry.app.into(), entry.id.into(), entry.owner.into(), title.as_str().into(), body.as_str().into(), entry.meta.to_string().into(), entry.at.into()],
    )
    .await?;
    if sqlite {
        let row = tx
            .query(
                "SELECT rowid FROM core_search WHERE app = $1 AND id = $2",
                &[entry.app.into(), entry.id.into()],
            )
            .await?;
        let rowid = row.first().and_then(|r| r.int(0)).unwrap_or(0);
        tx.execute(
            "INSERT INTO core_search_fts (rowid, title, body) VALUES ($1, $2, $3)",
            &[rowid.into(), title.as_str().into(), body.as_str().into()],
        )
        .await?;
    }
    tx.commit().await
}

/// Takes an item out of the index.
pub async fn remove(store: &dyn AsyncStore, app: &str, id: &str) -> StoreResult<()> {
    let mut tx = store.begin().await?;
    if store.engine() == Engine::Sqlite {
        let old = tx
            .query(
                "SELECT rowid, title, body FROM core_search WHERE app = $1 AND id = $2",
                &[app.into(), id.into()],
            )
            .await?;
        if let Some(r) = old.first() {
            tx.execute(
                "INSERT INTO core_search_fts (core_search_fts, rowid, title, body) VALUES ('delete', $1, $2, $3)",
                &[r.int(0).unwrap_or(0).into(), r.text(1).unwrap_or_default().into(), r.text(2).unwrap_or_default().into()],
            )
            .await?;
        }
    }
    tx.execute(
        "DELETE FROM core_search WHERE app = $1 AND id = $2",
        &[app.into(), id.into()],
    )
    .await?;
    tx.commit().await
}

/// Takes an item out of the index inside a transaction (a job's).
pub async fn remove_in(
    tx: &mut dyn crate::storage::AsyncTx,
    engine: Engine,
    app: &str,
    id: &str,
) -> StoreResult<()> {
    if engine == Engine::Sqlite {
        tx.execute(
            "INSERT INTO core_search_fts (core_search_fts, rowid, title, body) SELECT 'delete', rowid, title, body FROM core_search WHERE app = $1 AND id = $2",
            &[app.into(), id.into()],
        )
        .await?;
    }
    tx.execute(
        "DELETE FROM core_search WHERE app = $1 AND id = $2",
        &[app.into(), id.into()],
    )
    .await?;
    Ok(())
}

/// Which of `ids` (an app's items) are not in the index yet.
pub async fn missing(
    store: &dyn AsyncStore,
    app: &str,
    ids: &[String],
) -> StoreResult<Vec<String>> {
    let mut gone = Vec::new();
    for chunk in ids.chunks(200) {
        let marks: Vec<String> = (0..chunk.len()).map(|i| format!("${}", i + 2)).collect();
        let mut params: Vec<crate::storage::Value> = vec![app.into()];
        params.extend(chunk.iter().map(|id| id.as_str().into()));
        let found: std::collections::HashSet<String> = store
            .query(
                &format!(
                    "SELECT id FROM core_search WHERE app = $1 AND id IN ({})",
                    marks.join(", ")
                ),
                &params,
            )
            .await?
            .iter()
            .filter_map(|r| r.text(0).map(str::to_string))
            .collect();
        gone.extend(chunk.iter().filter(|id| !found.contains(*id)).cloned());
    }
    Ok(gone)
}

/// Moves items from one owner to another inside a transaction (a list or
/// project changing hands): whoever may see the new owner finds them.
pub async fn set_owner_in(
    tx: &mut dyn crate::storage::AsyncTx,
    app: &str,
    from: &str,
    to: &str,
    ids: &[String],
) -> StoreResult<()> {
    for id in ids {
        tx.execute(
            "UPDATE core_search SET owner = $1 WHERE app = $2 AND owner = $3 AND id = $4",
            &[to.into(), app.into(), from.into(), id.as_str().into()],
        )
        .await?;
    }
    Ok(())
}

/// Everything of an account's, for forgetting it.
pub async fn forget(store: &dyn AsyncStore, owner: &str) -> StoreResult<()> {
    let ids: Vec<(String, String)> = store
        .query(
            "SELECT app, id FROM core_search WHERE owner = $1",
            &[owner.into()],
        )
        .await?
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap_or_default().to_string(),
                r.text(1).unwrap_or_default().to_string(),
            )
        })
        .collect();
    for (app, id) in ids {
        remove(store, &app, &id).await?;
    }
    Ok(())
}

/// Everything of an account's, inside a transaction (an account being
/// forgotten).
pub async fn forget_in(
    tx: &mut dyn crate::storage::AsyncTx,
    engine: Engine,
    owner: &str,
) -> StoreResult<()> {
    if engine == Engine::Sqlite {
        tx.execute("INSERT INTO core_search_fts (core_search_fts, rowid, title, body) SELECT 'delete', rowid, title, body FROM core_search WHERE owner = $1", &[owner.into()]).await?;
    }
    tx.execute("DELETE FROM core_search WHERE owner = $1", &[owner.into()])
        .await?;
    Ok(())
}

/// After a restore (SQLite): the full-text index made again from the rows.
pub async fn rebuild(store: &dyn AsyncStore) -> StoreResult<()> {
    // Only where this database has the index (its schemas may not).
    let present = store.engine() == Engine::Sqlite
        && !store
            .query(
                "SELECT name FROM sqlite_master WHERE name = 'core_search_fts'",
                &[],
            )
            .await?
            .is_empty();
    if present {
        store
            .execute(
                "INSERT INTO core_search_fts (core_search_fts) VALUES ('rebuild')",
                &[],
            )
            .await?;
    }
    Ok(())
}

/// A search as typed: words, and filters (`from:anna`, `has:attachment`,
/// `in:mail`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub words: Vec<String>,
    pub from: Option<String>,
    pub attachment: bool,
    pub app: Option<String>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        let mut q = Query::default();
        for part in text.split_whitespace().take(20) {
            let lower = part.to_lowercase();
            if let Some(v) = lower.strip_prefix("from:") {
                q.from = Some(v.to_string()).filter(|v| !v.is_empty());
            } else if lower == "has:attachment" {
                q.attachment = true;
            } else if let Some(v) = lower.strip_prefix("in:") {
                q.app = Some(v.to_string()).filter(|v| !v.is_empty());
            } else {
                // Letters and digits only: nothing reaches the query language.
                let word: String = lower
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .take(64)
                    .collect();
                if !word.is_empty() {
                    q.words.push(word);
                }
            }
        }
        q
    }
}

/// One result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub app: String,
    pub id: String,
    pub owner: String,
    pub title: String,
    pub snippet: String,
    pub meta: Value,
    pub at: i64,
}

/// Text around the first word found.
fn snippet(body: &str, words: &[String]) -> String {
    let lower = body.to_lowercase();
    let at = words.iter().filter_map(|w| lower.find(w.as_str())).min();
    let chars: Vec<char> = body.chars().collect();
    let start_char = at
        .map(|byte| lower[..byte].chars().count())
        .unwrap_or(0)
        .saturating_sub(40);
    let text: String = chars.iter().skip(start_char).take(140).collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if start_char > 0 {
        format!("…{text}")
    } else {
        text
    }
}

/// The items of `owners` that match, newest first (up to `limit`).
pub async fn find(
    store: &dyn AsyncStore,
    owners: &[String],
    query: &Query,
    limit: i64,
) -> StoreResult<Vec<Hit>> {
    if owners.is_empty() {
        return Ok(Vec::new());
    }
    let mut params: Vec<crate::storage::Value> = Vec::new();
    let mut slots = Vec::new();
    for o in owners.iter().take(500) {
        params.push(o.as_str().into());
        slots.push(format!("${}", params.len()));
    }
    let mut filters = format!("s.owner IN ({})", slots.join(", "));
    if let Some(app) = &query.app {
        params.push(app.as_str().into());
        filters.push_str(&format!(" AND s.app = ${}", params.len()));
    }
    let sql = if query.words.is_empty() {
        format!("SELECT s.app, s.id, s.owner, s.title, s.body, s.meta, s.at FROM core_search s WHERE {filters} ORDER BY s.at DESC LIMIT {}", limit * 4)
    } else if store.engine() == Engine::Sqlite {
        let terms: Vec<String> = query.words.iter().map(|w| format!("\"{w}\"*")).collect();
        params.push(terms.join(" AND ").into());
        format!("SELECT s.app, s.id, s.owner, s.title, s.body, s.meta, s.at FROM core_search_fts f JOIN core_search s ON s.rowid = f.rowid WHERE core_search_fts MATCH ${} AND {filters} ORDER BY s.at DESC LIMIT {}", params.len(), limit * 4)
    } else {
        let terms: Vec<String> = query.words.iter().map(|w| format!("{w}:*")).collect();
        params.push(terms.join(" & ").into());
        format!("SELECT s.app, s.id, s.owner, s.title, s.body, s.meta, s.at FROM core_search s WHERE to_tsvector('simple', s.title || ' ' || s.body) @@ to_tsquery('simple', ${}) AND {filters} ORDER BY s.at DESC LIMIT {}", params.len(), limit * 4)
    };
    let rows = store.query(&sql, &params).await?;
    let mut out = Vec::new();
    for r in rows {
        let meta: Value = serde_json::from_str(r.text(5).unwrap_or("{}")).unwrap_or(Value::Null);
        if let Some(from) = &query.from {
            let who = meta["from"].as_str().unwrap_or_default().to_lowercase();
            if !who.contains(from.as_str()) {
                continue;
            }
        }
        if query.attachment && meta["attachment"] != json!(true) {
            continue;
        }
        out.push(Hit {
            app: r.text(0).unwrap_or_default().into(),
            id: r.text(1).unwrap_or_default().into(),
            owner: r.text(2).unwrap_or_default().into(),
            title: r.text(3).unwrap_or_default().into(),
            snippet: snippet(r.text(4).unwrap_or_default(), &query.words),
            meta,
            at: r.int(6).unwrap_or(0),
        });
        if out.len() as i64 >= limit {
            break;
        }
    }
    Ok(out)
}

/// What `user` may find: their own items, and others' that the apps vouch for.
pub async fn search(
    instance: &Instance,
    user: &str,
    query: &Query,
    limit: i64,
) -> StoreResult<Vec<Hit>> {
    let mut owners = vec![user.to_string()];
    for app in instance.apps.iter() {
        if !instance
            .switches
            .is_on(app.info().id)
            .await
            .unwrap_or(false)
        {
            continue;
        }
        for o in app.search_owners(instance, user).await {
            if !owners.contains(&o) {
                owners.push(o);
            }
        }
    }
    let found = find(instance.store.as_ref(), &owners, query, limit * 2).await?;
    // Others' items stay only where their app says so.
    let mut theirs: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for h in found.iter().filter(|h| h.owner != user) {
        theirs.entry(h.app.clone()).or_default().push(h.id.clone());
    }
    let mut allowed: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for (app_id, ids) in theirs {
        if let Some(app) = instance.app(&app_id) {
            for id in app.search_allowed(instance, user, ids).await {
                allowed.insert((app_id.clone(), id));
            }
        }
    }
    let mut out = Vec::new();
    for h in found {
        // Items of switched-off apps are not found.
        if !instance.switches.is_on(&h.app).await.unwrap_or(false) {
            continue;
        }
        if h.owner == user || allowed.contains(&(h.app.clone(), h.id.clone())) {
            out.push(h);
        }
        if out.len() as i64 >= limit {
            break;
        }
    }
    Ok(out)
}

async fn route(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let raw = request
        .request_uri
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or_default();
    let param = |name: &str| -> Option<String> {
        raw.split('&').find_map(|p| {
            let (k, v) = p.split_once('=')?;
            (k == name).then(|| crate::form::decode(v))
        })
    };
    let text = param("q").unwrap_or_default();
    if text.len() > 500 {
        return Err(ApiError::bad_request("The search is too long."));
    }
    let mut query = Query::parse(&text);
    if let Some(app) = param("app").filter(|a| !a.is_empty()) {
        query.app = Some(app);
    }
    if query.words.is_empty() && query.from.is_none() && !query.attachment {
        return Ok(ok(&json!({ "results": [] })));
    }
    let limit = param("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(30i64)
        .clamp(1, 100);
    let hits = search(instance, &me.user.id, &query, limit).await?;
    let results: Vec<Value> = hits.iter().map(|h| json!({"app": h.app, "id": h.id, "title": h.title, "snippet": h.snippet, "meta": h.meta, "at": h.at, "mine": h.owner == me.user.id})).collect();
    Ok(ok(&json!({ "results": results, "words": query.words })))
}

fn h_search(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, route(&request, &instance).await) })
}

pub fn routes() -> Vec<Route> {
    vec![Route {
        method: Method::Get,
        path: "/search",
        id: "search",
        tag: "search",
        summary: "Search one's mail, files, events and people: ?q= (words, from:name, has:attachment, in:app), ?app=, ?limit=.",
        access: Access::Signed,
        response: || json!({"type": "object"}),
        handler: h_search,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_read_safely() {
        let q = Query::parse("gauge from:Anna has:attachment \"report\" OR* in:mail");
        assert_eq!(vec!["gauge", "report", "or"], q.words);
        assert_eq!(
            (Some("anna".into()), true, Some("mail".into())),
            (q.from, q.attachment, q.app)
        );
        let found = snippet(
            &format!("{} the gauge at the bridge passed 4.2 m", "x ".repeat(40)),
            &["gauge".into()],
        );
        assert!(
            found.starts_with('…') && found.contains("gauge at the bridge passed 4.2 m"),
            "{found}"
        );
    }
}
