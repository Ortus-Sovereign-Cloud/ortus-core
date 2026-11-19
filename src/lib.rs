//! Ortus core: the part of Ortus every installation has.
//!
//! The core builds the rws application that the `ortus` programs serve.
//! Apps (mail, drive, chat and the rest) will register with it; for now it
//! answers the health check of the public API.

pub mod api;
pub mod config;
pub mod files;
pub mod jobs;
pub mod program;
pub mod runtime;
pub mod storage;
pub mod web;

use rust_web_server::middleware::WithMiddleware;
use rust_web_server::prelude::*;
use rust_web_server::request_id::RequestIdLayer;
use rust_web_server::state::AppWithState;

/// Version of the core, as reported by the health check.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The core application: the public API and the web client, with a
/// request id on every request. Nothing else is served, except the plain
/// `/healthz` probe that rws provides.
pub fn app() -> WithMiddleware<AppWithState<()>> {
    web::register(api::register(App::with_state(()), &api::routes()))
        .wrap(RequestIdLayer::new())
        .wrap(web::OnlyOrtusPaths)
}
