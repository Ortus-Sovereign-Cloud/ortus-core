//! Admin › Sovereignty & compliance: where data lives, who holds the keys,
//! who may reach us, the labels, how long things are kept, and the record
//! of everything; and the compliance report that says all of it at once.

use std::collections::HashSet;
use std::sync::Arc;

use rust_web_server::prelude::*;
use serde_json::{json, Value};

use crate::api::{answer, ok, ApiError, Method, Route};
use crate::auth::Access;
use crate::instance::Instance;
use crate::outbound::host_of;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture};

/// The hosts the owner configured: the only ones the program should call.
async fn configured_hosts(ortus: &Instance) -> Result<HashSet<String>, ApiError> {
    let store = ortus.store.as_ref();
    let mut hosts = HashSet::new();
    if let Some(mail) = &ortus.config.outgoing_mail {
        hosts.insert(host_of(&mail.host));
    }
    for (sql, purpose) in [
        ("SELECT issuer FROM core_external_providers", "sign_in"),
        ("SELECT sso_url FROM core_saml_providers", "sign_in"),
        ("SELECT url FROM core_directory", "directory"),
    ] {
        let _ = purpose;
        for r in store.query(sql, &[]).await? {
            hosts.insert(host_of(r.text(0).unwrap_or_default()));
        }
    }
    Ok(hosts)
}

async fn count(store: &dyn AsyncStore, sql: &str) -> Result<i64, ApiError> {
    Ok(store
        .query(sql, &[])
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0))
}

async fn facts(ortus: &Arc<Instance>) -> Result<Value, ApiError> {
    let store = ortus.store.as_ref();
    let (database, files) = crate::first_run::places(&ortus.config);
    let configured = configured_hosts(ortus).await?;
    let calls = crate::outbound::calls(store).await?;
    // A sign-in provider's token and key addresses come from its own
    // discovery document, so any host it sends us to counts as its own.
    let unexpected: Vec<Value> = calls
        .iter()
        .filter(|c| {
            c["purpose"] != "sign_in"
                && !configured.contains(c["host"].as_str().unwrap_or_default())
        })
        .cloned()
        .collect();
    let setting = |key: &'static str| async move { crate::auth::policy::setting(store, key).await };
    let policy = crate::policy::labels(store).await?;
    let partners: Vec<Value> = store
        .query("SELECT domain, name, channels FROM core_partners ORDER BY domain", &[])
        .await?
        .iter()
        .map(|r| json!({ "domain": r.text(0), "name": r.text(1), "channels": r.text(2).unwrap_or_default().split(',').filter(|c| !c.is_empty()).collect::<Vec<_>>() }))
        .collect();
    let retention = crate::retention::rules(store).await?;
    let mut periods: Vec<Value> = crate::retention::KINDS
        .iter()
        .map(|k| json!({ "kind": k, "days": retention.days.get(*k).copied().flatten() }))
        .collect();
    periods.sort_by_key(|p| {
        crate::retention::KINDS
            .iter()
            .position(|k| Some(*k) == p["kind"].as_str())
    });
    let holds: Vec<Value> = store
        .query("SELECT id, case_ref, created_at FROM core_holds WHERE released_at IS NULL ORDER BY created_at DESC", &[])
        .await?
        .iter()
        .map(|r| json!({ "id": r.text(0), "case_ref": r.text(1), "created_at": r.int(2) }))
        .collect();
    let audit_check = setting("audit_check")
        .await?
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    Ok(json!({
        "data": {
            "database": database,
            "files": files,
            "backup_taken_at": setting("backup_taken_at").await?.and_then(|t| t.parse::<i64>().ok()),
            "backup_checked_at": setting("backup_checked_at").await?.and_then(|t| t.parse::<i64>().ok()),
            "runs_on": if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() { "kubernetes" } else { "server" },
        },
        "outbound": { "calls": calls, "unexpected": unexpected, "checked_at": now_ms() },
        "keys": crate::secrets::facts(store, &ortus.keys).await?,
        "labels": policy,
        "partners": partners,
        "retention": { "periods": periods, "holds": holds, "people_on_hold": retention.held_people.len(), "channels_on_hold": retention.held_channels.len() },
        "audit": { "events": count(store, "SELECT COUNT(*) FROM core_audit").await?, "last_check": audit_check },
        "version": crate::VERSION,
    }))
}

async fn show(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    Ok(ok(&facts(ortus).await?))
}

/// The compliance report: the facts, checked now (the audit chain is
/// verified as part of it), as a file to keep.
async fn report(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let store = ortus.store.as_ref();
    let verified = crate::audit::verify(store).await?;
    let check =
        json!({ "at": now_ms(), "events": verified.events, "broken_at": verified.broken_at });
    crate::auth::policy::set_setting(store, "audit_check", &check.to_string()).await?;
    let mut report = facts(ortus).await?;
    report["audit"]["verified"] = check;
    report["generated_at"] = json!(now_ms());
    report["generated_by"] = json!(me.user.email);
    crate::auth::audit(
        ortus,
        crate::audit::Event::new("compliance.report", crate::audit::Outcome::Done).by(&me.user.id),
    )
    .await;
    let response = Response::json(
        rust_web_server::prelude::STATUS_CODE_REASON_PHRASE.n200_ok,
        serde_json::to_vec_pretty(&report).unwrap_or_default(),
    );
    Ok(crate::api::with_headers(
        response,
        vec![rust_web_server::header::Header {
            name: "Content-Disposition".to_string().into(),
            value: "attachment; filename=\"ortus-compliance-report.json\""
                .to_string()
                .into(),
        }],
    ))
}

fn h_show(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, show(&request, &ortus).await) })
}
fn h_report(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, report(&request, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Get, path: "/admin/sovereignty", id: "getSovereignty", tag: "admin", summary: "Where data lives, the hosts the program called (and any not configured), the master key's facts, labels, partners, retention and holds, and the audit log's size and last check.", access: Access::Admin, response: any_object, handler: h_show },
        Route { method: Method::Get, path: "/admin/sovereignty/report", id: "complianceReport", tag: "admin", summary: "The compliance report as a JSON file: the same facts, with the audit chain verified now.", access: Access::Admin, response: any_object, handler: h_report },
    ]
}
