//! Numbers for monitoring (Prometheus text format) at `GET
//! /api/v1/metrics`: the job queue by kind and state and how long the
//! oldest due job has waited, sessions, phones registered for
//! notifications, and how old the newest restore point is. Off unless
//! `ORTUS_METRICS_TOKEN` is set; then only with that token
//! (`Authorization: Bearer …`): the numbers say how busy the
//! installation is, which is not for everyone.

use std::sync::Arc;

use rust_web_server::prelude::*;

use crate::api::{answer, ApiError};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::BoxFuture;

fn allowed(request: &Request) -> bool {
    let Ok(expected) = std::env::var("ORTUS_METRICS_TOKEN") else {
        return false;
    };
    if expected.len() < 16 {
        return false;
    }
    let given = request
        .get_header("Authorization".to_string())
        .and_then(|h| h.value.strip_prefix("Bearer ").map(str::to_string))
        .unwrap_or_default();
    // Compared in constant time.
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

async fn metrics(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    if !allowed(request) {
        return Err(ApiError::not_found("There is nothing here."));
    }
    let store = instance.store.as_ref();
    let now = now_ms();
    let mut out = String::new();
    out.push_str(
        "# HELP ortus_jobs Jobs in the queue, by kind and state.\n# TYPE ortus_jobs gauge\n",
    );
    for r in store
        .query("SELECT kind, state, CAST(COUNT(*) AS BIGINT) FROM ortus_jobs GROUP BY kind, state ORDER BY kind, state", &[])
        .await?
    {
        out.push_str(&format!(
            "ortus_jobs{{kind=\"{}\",state=\"{}\"}} {}\n",
            label(r.text(0).unwrap_or_default()),
            label(r.text(1).unwrap_or_default()),
            r.int(2).unwrap_or(0)
        ));
    }
    let oldest = store
        .query(
            "SELECT MIN(run_at) FROM ortus_jobs WHERE state = 'queued' AND run_at <= $1",
            &[now.into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0));
    out.push_str("# HELP ortus_jobs_oldest_due_seconds How long the oldest due job has waited.\n# TYPE ortus_jobs_oldest_due_seconds gauge\n");
    out.push_str(&format!(
        "ortus_jobs_oldest_due_seconds {}\n",
        oldest.map_or(0, |at| (now - at).max(0) / 1000)
    ));
    let count = |sql: &'static str| async move {
        store
            .query(sql, &[])
            .await
            .ok()
            .and_then(|r| r.first().and_then(|r| r.int(0)))
            .unwrap_or(0)
    };
    out.push_str(
        "# HELP ortus_sessions Sessions that have not ended.\n# TYPE ortus_sessions gauge\n",
    );
    out.push_str(&format!(
        "ortus_sessions {}\n",
        count("SELECT CAST(COUNT(*) AS BIGINT) FROM core_sessions").await
    ));
    out.push_str("# HELP ortus_push_devices Phones registered for notifications.\n# TYPE ortus_push_devices gauge\n");
    out.push_str(&format!(
        "ortus_push_devices {}\n",
        count("SELECT CAST(COUNT(*) AS BIGINT) FROM core_push_devices").await
    ));
    let point = store
        .query("SELECT MAX(at) FROM core_points", &[])
        .await?
        .first()
        .and_then(|r| r.int(0));
    out.push_str("# HELP ortus_restore_point_age_seconds How old the newest restore point is (-1: none yet).\n# TYPE ortus_restore_point_age_seconds gauge\n");
    out.push_str(&format!(
        "ortus_restore_point_age_seconds {}\n",
        point.map_or(-1, |at| (now - at).max(0) / 1000)
    ));
    let mut response = Response::text(STATUS_CODE_REASON_PHRASE.n200_ok, &out);
    response
        .headers
        .retain(|h| !h.name.eq_ignore_ascii_case("Content-Type"));
    response.headers.push(rust_web_server::header::Header {
        name: "Content-Type".into(),
        value: "text/plain; version=0.0.4; charset=utf-8".into(),
    });
    Ok(response)
}

fn h_metrics(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, metrics(&request, &instance).await) })
}

pub fn routes() -> Vec<crate::api::Route> {
    vec![crate::api::Route {
        method: crate::api::Method::Get,
        path: "/metrics",
        id: "metrics",
        tag: "operations",
        summary: "Numbers for monitoring, in Prometheus text format: jobs by kind and state, the oldest due job's wait, sessions, phones, the newest restore point's age. Only with ORTUS_METRICS_TOKEN as a bearer token.",
        access: crate::auth::Access::Public,
        response: || serde_json::json!({"type": "string"}),
        handler: h_metrics,
    }]
}
