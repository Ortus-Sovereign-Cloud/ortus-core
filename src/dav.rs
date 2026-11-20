//! A mount for WebDAV apps (CalDAV, CardDAV): every WebDAV method on
//! `/dav/<app id>/…` goes to that app's [`DavHandler`], and the
//! `/.well-known/` names apps claim redirect there (RFC 6764). Nothing
//! else under `/dav/` or `/.well-known/` is served.

use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;
use rust_web_server::handler::{AsyncHandler, HandlerFuture, RouteSpec};
use rust_web_server::header::Header;
use rust_web_server::prelude::*;

use crate::instance::Instance;
use crate::storage::BoxFuture;

/// An app's WebDAV handler: the request, and the path after `/dav/<id>`
/// (starting with `/`, or empty).
pub type DavHandler =
    fn(Request, ConnectionInfo, Arc<Instance>, String) -> BoxFuture<'static, Response>;

fn not_found() -> Response {
    Response::text(STATUS_CODE_REASON_PHRASE.n404_not_found, "Not found")
}

/// Sends a request under `/dav/` to the app it names. (rws hands routes
/// their state in an `Arc`; the state is the `Arc<Instance>` itself.)
#[allow(clippy::redundant_allocation)]
async fn dispatch(
    request: Request,
    params: PathParams,
    connection: ConnectionInfo,
    state: Arc<Arc<Instance>>,
) -> Response {
    let instance = Arc::clone(&*state);
    let id = params.get("app").unwrap_or_default().to_string();
    // From the request itself: a collection's trailing slash counts.
    let path = request
        .request_uri
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let rest = path
        .strip_prefix(&format!("/dav/{id}"))
        .unwrap_or_default()
        .to_string();
    let Some(app) = instance.apps.iter().find(|a| a.info().id == id) else {
        return not_found();
    };
    let Some(handler) = app.dav() else {
        return not_found();
    };
    if !instance.switches.is_on(&id).await.unwrap_or(false) {
        return not_found();
    }
    handler(request, connection, instance, rest).await
}

macro_rules! dav_method {
    ($name:ident, $method:literal) => {
        struct $name;
        impl RouteSpec for $name {
            const METHOD: &'static str = $method;
            const PATTERN: &'static str = "/dav/:app/*path";
        }
        impl AsyncHandler<Arc<Instance>> for $name {
            fn handle_async(
                &self,
                request: Request,
                params: PathParams,
                connection: ConnectionInfo,
                state: Arc<Arc<Instance>>,
            ) -> HandlerFuture {
                Box::pin(dispatch(request, params, connection, state))
            }
        }
    };
}

dav_method!(Options, "OPTIONS");
dav_method!(Propfind, "PROPFIND");
dav_method!(Proppatch, "PROPPATCH");
dav_method!(Report, "REPORT");
dav_method!(Mkcalendar, "MKCALENDAR");
dav_method!(Mkcol, "MKCOL");
dav_method!(Move, "MOVE");
dav_method!(Copy, "COPY");

/// `/.well-known/<name>` answers for PROPFIND as for GET: a redirect.
struct WellKnownPropfind;
impl RouteSpec for WellKnownPropfind {
    const METHOD: &'static str = "PROPFIND";
    const PATTERN: &'static str = "/.well-known/:name";
}
impl AsyncHandler<Arc<Instance>> for WellKnownPropfind {
    fn handle_async(
        &self,
        request: Request,
        params: PathParams,
        _: ConnectionInfo,
        state: Arc<Arc<Instance>>,
    ) -> HandlerFuture {
        Box::pin(async move { well_known(&request, &params, &state) })
    }
}

fn well_known(_: &Request, params: &PathParams, instance: &Instance) -> Response {
    let name = params.get("name").unwrap_or_default();
    let target = instance
        .apps
        .iter()
        .flat_map(|a| a.well_known())
        .find(|(n, _)| *n == name)
        .map(|(_, t)| t);
    match target {
        Some(target) => {
            let mut response = Response::text(STATUS_CODE_REASON_PHRASE.n301_moved_permanently, "");
            response.headers.push(Header {
                name: "Location".to_string().into(),
                value: target.to_string().into(),
            });
            response
        }
        None => not_found(),
    }
}

/// Adds the WebDAV mount and the well-known redirects.
pub fn register(
    mut app: AsyncAppWithState<Arc<Instance>>,
    apps: &[Arc<dyn crate::app::App>],
) -> AsyncAppWithState<Arc<Instance>> {
    if !apps.iter().any(|a| a.dav().is_some()) {
        return app;
    }
    let pattern = "/dav/:app/*path";
    app = app
        .get(pattern, dispatch)
        .put(pattern, dispatch)
        .delete(pattern, dispatch)
        .post(pattern, dispatch);
    app = app
        .route(Options)
        .route(Propfind)
        .route(Proppatch)
        .route(Report)
        .route(Mkcalendar)
        .route(Mkcol)
        .route(Move)
        .route(Copy);
    app.get(
        "/.well-known/:name",
        |request, params, _, state: Arc<Arc<Instance>>| async move {
            well_known(&request, &params, &state)
        },
    )
    .route(WellKnownPropfind)
}

/// Paths the WebDAV mount serves, for the path guard: `/dav/<id>…` of apps
/// with a handler, and the well-known names they claim.
pub fn served(path: &str, dav_ids: &[String], well_known: &[String]) -> bool {
    if let Some(rest) = path.strip_prefix("/dav/") {
        let id = rest.split('/').next().unwrap_or_default();
        return dav_ids.iter().any(|d| d == id);
    }
    path.strip_prefix("/.well-known/")
        .is_some_and(|name| well_known.iter().any(|w| w == name))
}
