//! The admin overview: the six steps that take a new installation live,
//! each worked out from what is really there, and the installation's
//! facts (where it runs, its version, where data lives, what it calls out
//! to).

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde_json::{json, Value};

use crate::api::{answer, ok, ApiError, Method, Route};
use crate::auth::policy::{set_setting, setting};
use crate::auth::Access;
use crate::config::{Database, FileStore};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, StoreResult};

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

/// The host of a URL, without user, password or path.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    rest.split(['/', '?'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Whether the public address is a real one: HTTPS and not this machine.
fn public_address(url: &str) -> bool {
    let host = host_of(url);
    let name = host
        .rsplit_once(':')
        .map_or(host.as_str(), |(name, _)| name);
    url.starts_with("https://")
        && !matches!(name, "localhost" | "127.0.0.1" | "[::1]")
        && !name.ends_with(".localhost")
}

/// Where the database and files are, without secrets.
pub fn places(config: &crate::config::Config) -> (Value, Value) {
    let database = match &config.database {
        Database::Sqlite { .. } => json!({ "engine": "sqlite", "where": "this server" }),
        Database::Postgres { url, .. } => json!({ "engine": "postgres", "where": host_of(url) }),
    };
    let files = match &config.files {
        FileStore::Local { .. } => json!({ "engine": "local", "where": "this server" }),
        FileStore::S3(s3) => {
            json!({ "engine": "s3", "where": format!("{} / {}", host_of(&s3.endpoint), s3.bucket) })
        }
    };
    (database, files)
}

async fn overview(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let store = ortus.store.as_ref();
    let config = &ortus.config;

    let kit = count(
        store,
        "SELECT COUNT(*) FROM core_credentials WHERE user_id = $1 AND kind = 'recovery'",
        &[me.user.id.as_str().into()],
    )
    .await?
        > 0;
    let people = count(store, "SELECT COUNT(*) FROM core_users", &[]).await?;
    let directory = count(store, "SELECT COUNT(*) FROM core_directory", &[]).await? > 0;
    let policy = setting(store, "sign_in_policy").await?.is_some();
    let data = setting(store, "data_place_confirmed").await?.is_some();
    let checked = setting(store, "backup_checked_at")
        .await?
        .and_then(|t| t.parse::<i64>().ok());
    let taken = setting(store, "backup_taken_at")
        .await?
        .and_then(|t| t.parse::<i64>().ok());
    let steps = [
        ("recovery_kit", kit),
        ("domain", public_address(&config.public_url)),
        ("people", people > 1 || directory),
        ("sign_in", policy),
        ("data", data),
        ("restore", checked.is_some()),
    ];
    let next = steps.iter().position(|(_, done)| !done);
    let steps: Vec<Value> = steps
        .iter()
        .enumerate()
        .map(|(i, (id, done))| json!({ "id": id, "state": if *done { "done" } else if Some(i) == next { "next" } else { "later" } }))
        .collect();

    let mut outbound = Vec::new();
    if let Some(mail) = &config.outgoing_mail {
        outbound.push(json!({ "what": "mail", "host": mail.host }));
    }
    for row in store
        .query(
            "SELECT issuer FROM core_external_providers ORDER BY name",
            &[],
        )
        .await?
    {
        outbound
            .push(json!({ "what": "sign_in", "host": host_of(row.text(0).unwrap_or_default()) }));
    }
    for row in store
        .query("SELECT sso_url FROM core_saml_providers ORDER BY name", &[])
        .await?
    {
        outbound
            .push(json!({ "what": "sign_in", "host": host_of(row.text(0).unwrap_or_default()) }));
    }
    for row in store.query("SELECT url FROM core_directory", &[]).await? {
        outbound
            .push(json!({ "what": "directory", "host": host_of(row.text(0).unwrap_or_default()) }));
    }
    let (database, files) = places(config);
    Ok(ok(&json!({
        "name": me.user.name,
        "steps": steps,
        "done": steps.iter().filter(|s| s["state"] == "done").count(),
        "public_url": config.public_url,
        "owners": count(store, "SELECT COUNT(*) FROM core_users WHERE role = 'owner' AND state <> 'suspended'", &[]).await?,
        "installation": {
            "runs_on": if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() { "kubernetes" } else { "server" },
            "version": crate::VERSION,
            "healthy": true,
            "database": database,
            "files": files,
            "outbound": outbound,
            "backup_taken_at": taken,
            "backup_checked_at": checked,
        },
    })))
}

/// The owner confirms where data lives (the one step only a person can judge).
async fn confirm_data(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    set_setting(
        ortus.store.as_ref(),
        "data_place_confirmed",
        &now_ms().to_string(),
    )
    .await?;
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("admin.data_place_confirmed", crate::audit::Outcome::Done)
            .by(&me.user.id),
    )
    .await;
    overview(request, ortus).await
}

fn h_overview(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, overview(&request, &ortus).await) })
}
fn h_confirm(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, confirm_data(&request, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Get, path: "/admin/overview", id: "getAdminOverview", tag: "admin", summary: "The steps to go live (done, next, later) and the installation's facts: where it runs, version, where data lives, what it calls out to.", access: Access::Admin, response: any_object, handler: h_overview },
        Route { method: Method::Post, path: "/admin/overview/data-confirmed", id: "confirmDataPlace", tag: "admin", summary: "The owner confirms where data lives.", access: Access::Owner, response: any_object, handler: h_confirm },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_leave_secrets_out() {
        assert_eq!(
            "db.internal:5432",
            host_of("postgres://ortus:secret@db.internal:5432/ortus?sslmode=require")
        );
        assert_eq!(
            "idp.example.org",
            host_of("https://idp.example.org/realms/x")
        );
        assert_eq!(
            "ldap.example.org:636",
            host_of("ldaps://ldap.example.org:636")
        );
    }

    #[test]
    fn a_public_address_is_https_and_not_this_machine() {
        assert!(public_address("https://ortus.example.org"));
        assert!(!public_address("http://ortus.example.org"));
        assert!(!public_address("https://localhost:7979"));
        assert!(!public_address("https://127.0.0.1"));
    }
}
