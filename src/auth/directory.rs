//! Bringing people and groups in from the organisation's directory
//! (LDAP or Active Directory): the owner sets the connection once, and an
//! administrator syncs when they like. New people come in invited; names
//! of people already here are brought up to date; groups follow the
//! directory's groupOfNames (or group) entries.

use std::collections::HashMap;
use std::sync::Arc;

use ldap3::{LdapConnAsync, LdapConnSettings, Scope, SearchEntry};
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, Field, NewUser, Role, State};
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{BoxFuture, Migration, Schema};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "directory connection",
    sql: "CREATE TABLE core_directory (
        id BIGINT PRIMARY KEY,
        url TEXT NOT NULL,
        bind_dn TEXT NOT NULL,
        password TEXT NOT NULL,
        base_dn TEXT NOT NULL,
        user_filter TEXT NOT NULL,
        group_filter TEXT NOT NULL,
        mail_attribute TEXT NOT NULL,
        name_attribute TEXT NOT NULL,
        ca_certificate TEXT,
        last_sync_at BIGINT,
        last_result TEXT
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.directory",
    migrations: MIGRATIONS,
};

#[derive(Clone, Deserialize)]
struct Settings {
    url: String,
    bind_dn: String,
    /// Kept as it was when left out.
    password: Option<String>,
    base_dn: String,
    user_filter: Option<String>,
    group_filter: Option<String>,
    mail_attribute: Option<String>,
    name_attribute: Option<String>,
    ca_certificate: Option<String>,
}

struct Stored {
    url: String,
    bind_dn: String,
    password: String,
    base_dn: String,
    user_filter: String,
    group_filter: String,
    mail_attribute: String,
    name_attribute: String,
    ca_certificate: Option<String>,
}

async fn stored(ortus: &Instance) -> Result<Option<Stored>, ApiError> {
    let rows = ortus
        .store
        .query("SELECT url, bind_dn, password, base_dn, user_filter, group_filter, mail_attribute, name_attribute, ca_certificate FROM core_directory WHERE id = 1", &[])
        .await?;
    Ok(rows.first().map(|r| {
        let t = |i: usize| r.text(i).unwrap_or_default().to_string();
        Stored {
            url: t(0),
            bind_dn: t(1),
            password: t(2),
            base_dn: t(3),
            user_filter: t(4),
            group_filter: t(5),
            mail_attribute: t(6),
            name_attribute: t(7),
            ca_certificate: r.text(8).map(str::to_string),
        }
    }))
}

/// Connects and binds; the caller searches.
async fn connect(settings: &Stored) -> Result<ldap3::Ldap, String> {
    if !(settings.url.starts_with("ldap://") || settings.url.starts_with("ldaps://")) {
        return Err("the address must start with ldap:// or ldaps://".into());
    }
    let tls = crate::http::tls_config(settings.ca_certificate.as_deref())?;
    let connection = LdapConnSettings::new()
        .set_config(Arc::new(tls))
        .set_conn_timeout(std::time::Duration::from_secs(15));
    let (conn, mut ldap) = LdapConnAsync::with_settings(connection, &settings.url)
        .await
        .map_err(|e| format!("cannot reach the directory: {e}"))?;
    ldap3::drive!(conn);
    ldap.simple_bind(&settings.bind_dn, &settings.password)
        .await
        .and_then(|r| r.success())
        .map_err(|e| format!("the directory refused the sign-in: {e}"))?;
    Ok(ldap)
}

async fn search(
    ldap: &mut ldap3::Ldap,
    base: &str,
    filter: &str,
    attributes: Vec<&str>,
) -> Result<Vec<SearchEntry>, String> {
    let (entries, _) = ldap
        .search(base, Scope::Subtree, filter, attributes)
        .await
        .and_then(|r| r.success())
        .map_err(|e| format!("the search failed: {e}"))?;
    Ok(entries.into_iter().map(SearchEntry::construct).collect())
}

fn first(entry: &SearchEntry, attribute: &str) -> Option<String> {
    entry
        .attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(attribute))
        .and_then(|(_, values)| values.first().cloned())
}

async fn save(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: Settings = body(request)?;
    let previous = stored(ortus).await?;
    let settings = Stored {
        url: input.url.trim().to_string(),
        bind_dn: input.bind_dn.trim().to_string(),
        password: input
            .password
            .filter(|p| !p.is_empty())
            .or(previous.map(|p| p.password))
            .unwrap_or_default(),
        base_dn: input.base_dn.trim().to_string(),
        user_filter: input
            .user_filter
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| "(objectClass=inetOrgPerson)".into()),
        group_filter: input
            .group_filter
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| "(objectClass=groupOfNames)".into()),
        mail_attribute: input
            .mail_attribute
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| "mail".into()),
        name_attribute: input
            .name_attribute
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| "cn".into()),
        ca_certificate: input.ca_certificate.filter(|c| !c.trim().is_empty()),
    };
    // Reachable, and the sign-in works, before it is kept.
    let mut ldap = connect(&settings).await.map_err(ApiError::bad_request)?;
    let _ = ldap.unbind().await;
    let mut tx = ortus.store.begin().await?;
    tx.execute("DELETE FROM core_directory", &[]).await?;
    tx.execute(
        "INSERT INTO core_directory (id, url, bind_dn, password, base_dn, user_filter, group_filter, mail_attribute, name_attribute, ca_certificate) VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &[
            settings.url.as_str().into(),
            settings.bind_dn.as_str().into(),
            settings.password.as_str().into(),
            settings.base_dn.as_str().into(),
            settings.user_filter.as_str().into(),
            settings.group_filter.as_str().into(),
            settings.mail_attribute.as_str().into(),
            settings.name_attribute.as_str().into(),
            settings.ca_certificate.clone().into(),
        ],
    )
    .await?;
    tx.commit().await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.directory_connected", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(json!({"url": settings.url, "base_dn": settings.base_dn})),
    )
    .await;
    show(request, ortus).await
}

async fn show(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let rows = ortus.store.query("SELECT url, bind_dn, base_dn, user_filter, group_filter, mail_attribute, name_attribute, last_sync_at, last_result FROM core_directory WHERE id = 1", &[]).await?;
    let Some(r) = rows.first() else {
        return Ok(ok(&json!({ "connected": false })));
    };
    Ok(ok(&json!({
        "connected": true, "url": r.text(0), "bind_dn": r.text(1), "base_dn": r.text(2), "user_filter": r.text(3), "group_filter": r.text(4),
        "mail_attribute": r.text(5), "name_attribute": r.text(6), "last_sync_at": r.int(7),
        "last_result": r.text(8).and_then(|t| serde_json::from_str::<Value>(t).ok()),
    })))
}

async fn sync(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let settings = stored(ortus)
        .await?
        .ok_or_else(|| ApiError::bad_request("No directory is connected."))?;
    let mut ldap = connect(&settings).await.map_err(ApiError::bad_request)?;
    let people = search(
        &mut ldap,
        &settings.base_dn,
        &settings.user_filter,
        vec![
            settings.mail_attribute.as_str(),
            settings.name_attribute.as_str(),
        ],
    )
    .await
    .map_err(ApiError::bad_request)?;
    let groups = search(
        &mut ldap,
        &settings.base_dn,
        &settings.group_filter,
        vec!["cn", "member", "uniqueMember"],
    )
    .await
    .map_err(ApiError::bad_request)?;
    let _ = ldap.unbind().await;

    let store = ortus.store.as_ref();
    let (mut created, mut updated, mut skipped) = (0, 0, 0);
    let mut by_dn: HashMap<String, String> = HashMap::new();
    for entry in &people {
        let (Some(mail), name) = (
            first(entry, &settings.mail_attribute).and_then(|m| accounts::normal_email(&m)),
            first(entry, &settings.name_attribute),
        ) else {
            skipped += 1;
            continue;
        };
        let name = name.unwrap_or_else(|| mail.clone());
        let user = match accounts::by_email(store, &mail).await? {
            Some(user) => {
                if user.name != name {
                    accounts::set(store, &user.id, Field::Name(&name)).await?;
                    updated += 1;
                }
                user
            }
            None => {
                created += 1;
                accounts::create(
                    store,
                    &NewUser {
                        email: &mail,
                        name: &name,
                        role: Role::Member,
                        state: State::Invited,
                        expires_at: None,
                    },
                    now_ms(),
                )
                .await?
            }
        };
        by_dn.insert(entry.dn.to_lowercase(), user.id);
    }
    let mut group_names = Vec::new();
    for group in &groups {
        let Some(name) = first(group, "cn") else {
            continue;
        };
        let members: Vec<String> = group
            .attrs
            .iter()
            .filter(|(attribute, _)| {
                attribute.eq_ignore_ascii_case("member")
                    || attribute.eq_ignore_ascii_case("uniqueMember")
            })
            .flat_map(|(_, values)| values.iter().map(|v| v.to_lowercase()))
            .collect();
        for member in members {
            if let Some(user_id) = by_dn.get(&member) {
                super::admin_users::join_group(store, &name, user_id).await?;
            }
        }
        group_names.push(name);
    }
    let result = json!({ "created": created, "updated": updated, "skipped": skipped, "groups": group_names.len() });
    store
        .execute(
            "UPDATE core_directory SET last_sync_at = $1, last_result = $2 WHERE id = 1",
            &[now_ms().into(), result.to_string().into()],
        )
        .await?;
    super::audit(
        ortus,
        crate::audit::Event::new("people.directory_synced", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .with(result.clone()),
    )
    .await;
    Ok(ok(&result))
}

fn h_save(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, save(&request, &ortus).await) })
}
fn h_show(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, show(&request, &ortus).await) })
}
fn h_sync(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, sync(&request, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Put, path: "/admin/directory", id: "connectDirectory", tag: "people", summary: "Connect the organisation's directory: {url, bind_dn, password, base_dn, user_filter, group_filter, mail_attribute, name_attribute, ca_certificate}. Checked before it is kept.", access: Access::Owner, response: any_object, handler: h_save },
        Route { method: Method::Get, path: "/admin/directory", id: "getDirectory", tag: "people", summary: "The directory connection (without its password) and the last sync.", access: Access::Admin, response: any_object, handler: h_show },
        Route { method: Method::Post, path: "/admin/directory/sync", id: "syncDirectory", tag: "people", summary: "Bring people and groups in from the directory now.", access: Access::Admin, response: any_object, handler: h_sync },
    ]
}
