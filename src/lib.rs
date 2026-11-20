//! Ortus core: the part of Ortus every installation has.
//!
//! The core builds the rws application that the `ortus` programs serve.
//! Apps (mail, drive, chat and the rest) will register with it; for now it
//! answers the health check of the public API.

pub mod api;
pub mod app;
pub mod auth;
pub mod backup;
pub mod cli;
pub mod config;
pub mod files;
pub mod instance;
pub mod interfaces;
pub mod jobs;
pub mod program;
pub mod runtime;
pub mod services;
pub mod storage;
pub mod web;

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
    ]
}

/// The web application of an installation: the public API and the web
/// client of the core and its apps, with a request id on every request.
/// Nothing else is served, except the health probes that rws provides
/// (`/healthz`, `/livez`, `/readyz`).
pub fn app(instance: Arc<Instance>) -> WithMiddleware<AsyncAppWithState<Arc<Instance>>> {
    let apps = instance.apps.clone();
    let paths = apps.iter().map(|a| a.info().path.to_string()).collect();
    web::register(
        api::register(AsyncAppWithState::new(instance), &apps),
        &apps,
    )
    .wrap(RequestIdLayer::new())
    .wrap(web::OnlyOrtusPaths::new(paths))
}
