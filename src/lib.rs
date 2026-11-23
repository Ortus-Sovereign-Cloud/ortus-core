//! Ortus core: the part of Ortus every installation has.
//!
//! The core builds the rws application that the `ortus` programs serve.
//! Apps (mail, drive, chat and the rest) will register with it; for now it
//! answers the health check of the public API.

pub mod api;
pub mod app;
pub mod app_links;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod cli;
pub mod config;
pub mod dav;
pub mod files;
pub mod first_run;
pub mod form;
pub mod http;
pub mod instance;
pub mod interfaces;
pub mod jobs;
pub mod mail_out;
pub mod notify;
pub mod oidc;
pub mod outbound;
pub mod points;
pub mod policy;
pub mod program;
pub mod push;
pub mod retention;
pub mod runtime;
pub mod search;
pub mod secrets;
pub mod services;
pub mod sovereignty;
pub mod storage;
pub mod web;
pub mod webhooks;

use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;
use rust_web_server::middleware::WithMiddleware;
use rust_web_server::request_id::RequestIdLayer;

use crate::instance::Instance;

/// Version of the core, as reported by the health check.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every database schema of the core, in the order they are installed.
/// Apps add their own after these.
pub fn schemas() -> Vec<storage::Schema> {
    vec![
        jobs::SCHEMA,
        app::SWITCHES_SCHEMA,
        auth::SCHEMA,
        auth::setup::SCHEMA,
        auth::admin_users::SCHEMA,
        auth::directory::SCHEMA,
        auth::policy::SCHEMA,
        auth::personal::SCHEMA,
        auth::photos::SCHEMA,
        auth::phone_link::SCHEMA,
        policy::SCHEMA,
        retention::SCHEMA,
        outbound::SCHEMA,
        audit::SCHEMA,
        oidc::SCHEMA,
        auth::external::SCHEMA,
        auth::saml::SCHEMA,
        search::SCHEMA,
        notify::SCHEMA,
        push::SCHEMA,
        points::SCHEMA,
        webhooks::SCHEMA,
    ]
}

/// The kinds of background job of the core.
pub fn core_jobs(instance: &Arc<Instance>) -> Vec<app::JobKind> {
    let mut all: Vec<app::JobKind> = mail_out::job(instance.config.outgoing_mail.clone())
        .into_iter()
        .collect();
    all.push(auth::signup::expiry_job(Arc::clone(instance)));
    all.push(retention::sweep_job(Arc::clone(instance)));
    all.push(jobs::prune_job());
    all
}

/// Jobs the core runs on a schedule: (kind, every so many milliseconds).
pub const PERIODIC: &[(&str, i64)] = &[
    (auth::signup::EXPIRY_JOB, 60 * 60 * 1000),
    (retention::SWEEP_JOB, 24 * 60 * 60 * 1000),
    (jobs::PRUNE_JOB, 24 * 60 * 60 * 1000),
    // Restore points: looks every ten minutes, takes one an hour.
    (points::JOB, 10 * 60 * 1000),
    // Outgoing webhooks: new events handed out and posted.
    (webhooks::JOB, 60 * 1000),
];

/// Jobs of the core that run outside a transaction (long ones).
pub fn core_outside_jobs(instance: &Arc<Instance>) -> Vec<app::OutsideJobKind> {
    vec![
        points::job(Arc::clone(instance)),
        points::test_job(Arc::clone(instance)),
        webhooks::job(Arc::clone(instance)),
        push::job(Arc::clone(instance)),
    ]
}

/// The web application of an installation: the public API and the web
/// client of the core and its apps, with a request id on every request.
/// Nothing else is served, except the health probes that rws provides
/// (`/healthz`, `/livez`, `/readyz`).
pub fn app(instance: Arc<Instance>) -> WithMiddleware<AsyncAppWithState<Arc<Instance>>> {
    let apps = instance.apps.clone();
    let paths = apps.iter().map(|a| a.info().path.to_string()).collect();
    let dav_ids = apps
        .iter()
        .filter(|a| a.dav().is_some())
        .map(|a| a.info().id.to_string())
        .collect();
    let well_known = apps
        .iter()
        .flat_map(|a| a.well_known())
        .map(|(name, _)| name.to_string())
        .collect();
    let routes = api::register(AsyncAppWithState::new(instance), &apps);
    let routes = auth::saml::register(auth::external::register(oidc::routes::register(routes)));
    let routes = app_links::register(routes);
    let routes = dav::register(routes, &apps);
    web::register(routes, &apps)
        .wrap(RequestIdLayer::new())
        .wrap(web::OnlyOrtusPaths::new(paths).with_dav(dav_ids, well_known))
}
