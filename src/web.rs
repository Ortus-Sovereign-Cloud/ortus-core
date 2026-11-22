//! The web client, served by the program itself.
//!
//! The client is built by Vite into `web/dist` and embedded in the binary
//! at compile time, so one file is the whole installation. Files under
//! `/assets/` carry a content hash in their name and are cached for a
//! year; the page itself is always revalidated, so an upgrade shows at the
//! next load.

use rust_web_server::application::Application;
use rust_web_server::header::Header;
use rust_web_server::middleware::Middleware;
use rust_web_server::prelude::*;
use rust_web_server::range::Range;
use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;

use crate::instance::Instance;

#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
struct Client;

/// What the page may load and do. Everything comes from this server:
/// no inline script or style, no outside origin, no framing.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'";

/// Paths of the web client that are screens, not files: each answers
/// with the page, and the client shows the screen for the path. Every app
/// adds its own path and everything under it.
pub const SCREENS: &[&str] = &[
    "/",
    "/setup",
    "/sign-in",
    "/sign-up",
    "/terms",
    // What the app stores link to: the apps' privacy policy and support.
    "/privacy",
    "/support",
    "/welcome",
    // The shared app link of an invitation (app_links).
    "/i",
    "/apps",
    "/settings",
    "/admin",
];

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "json" => "application/json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.to_string().into(),
        value: value.to_string().into(),
    }
}

fn file(path: &str, cache: &str) -> Option<Response> {
    let embedded = Client::get(path)?;
    Some(respond(path, embedded.data.into_owned(), cache))
}

/// A file's response, with the headers every Ortus file carries.
fn respond(path: &str, data: Vec<u8>, cache: &str) -> Response {
    let headers = vec![
        header("Cache-Control", cache),
        header("X-Content-Type-Options", "nosniff"),
        header("Content-Security-Policy", CONTENT_SECURITY_POLICY),
        header("Referrer-Policy", "no-referrer"),
        header("Cross-Origin-Opener-Policy", "same-origin"),
    ];
    let body = Range::get_content_range(data, mime(path).to_string());
    Response::get_response(
        STATUS_CODE_REASON_PHRASE.n200_ok,
        Some(headers),
        Some(vec![body]),
    )
}

/// Files with a fixed name (the kit, an app's entry) change with each
/// release, so browsers ask again; hashed names are kept for a year.
fn cache_for(path: &str) -> &'static str {
    if path == "kit.js" || path == "app.js" || path.ends_with("/kit.js") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}

fn not_found() -> Response {
    Response::text(STATUS_CODE_REASON_PHRASE.n404_not_found, "Not found")
}

async fn page() -> Response {
    file("index.html", "no-cache").unwrap_or_else(not_found)
}

/// Registers the web client on `app`: the core's screens, every app's
/// screens, and the embedded files.
pub fn register(
    mut app: AsyncAppWithState<Arc<Instance>>,
    apps: &[Arc<dyn crate::app::App>],
) -> AsyncAppWithState<Arc<Instance>> {
    let mut screens: Vec<String> = SCREENS.iter().map(|s| s.to_string()).collect();
    for screen in SCREENS.iter().skip(1) {
        screens.push(format!("{screen}/*rest"));
    }
    let mut app_paths: Vec<&str> = KNOWN_APP_PATHS.to_vec();
    for owner in apps {
        let path = owner.info().path;
        if !app_paths.contains(&path) {
            app_paths.push(path);
        }
    }
    for path in app_paths {
        screens.push(path.to_string());
        screens.push(format!("{path}/*rest"));
    }
    for screen in &screens {
        app = app.get(screen, |_, _, _, _| page());
    }
    app.get(
        "/assets/apps/:id/*path",
        |_, params: PathParams, _, instance: Arc<Arc<Instance>>| async move {
            // An app's own built client, from the app itself.
            let id = params.get("id").unwrap_or_default();
            let path = params.get("path").unwrap_or_default();
            match instance.app(id).and_then(|app| app.web_file(path)) {
                Some(data) => respond(path, data.into_owned(), cache_for(path)),
                None => not_found(),
            }
        },
    )
    .get("/assets/*path", |_, params: PathParams, _, _| async move {
        let path = params.get("path").unwrap_or_default().to_string();
        // Embedded names only: a path with `..` simply has no such file.
        file(&format!("assets/{path}"), cache_for(&path)).unwrap_or_else(not_found)
    })
    .get("/favicon.svg", |_, _, _, _| async {
        file("favicon.svg", "public, max-age=86400").unwrap_or_else(not_found)
    })
}

/// The health probes rws answers: liveness (`/healthz`, `/livez`) and
/// readiness (`/readyz`, which fails while the program shuts down).
pub const PROBES: &[&str] = &["/healthz", "/livez", "/readyz"];

/// Where the apps of Ortus live. The page is served under each of them
/// even when the app is not installed, so the client can say so instead of
/// the server answering "Not found".
pub const KNOWN_APP_PATHS: &[&str] = &[
    "/mail",
    "/calendar",
    "/people",
    "/drive",
    "/docs",
    "/chat",
    "/calls",
    "/tasks",
    "/approvals",
];

/// Lets through only the paths Ortus serves, and refuses the rest.
///
/// rws serves any file under the program's working directory when no
/// route matches. For Ortus that directory can hold the database and the
/// configuration, so nothing may reach that fallback: a request must be
/// for a screen, an embedded file, the API or the health probe.
pub struct OnlyOrtusPaths {
    app_paths: Vec<String>,
    dav_ids: Vec<String>,
    well_known: Vec<String>,
}

impl OnlyOrtusPaths {
    /// For the core and apps living at `app_paths` (as `/mail`).
    pub fn new(app_paths: Vec<String>) -> Self {
        Self {
            app_paths,
            dav_ids: Vec::new(),
            well_known: Vec::new(),
        }
    }

    /// Also `/dav/<id>…` of these apps, and these `/.well-known/` names.
    pub fn with_dav(mut self, dav_ids: Vec<String>, well_known: Vec<String>) -> Self {
        self.dav_ids = dav_ids;
        self.well_known = well_known;
        self
    }
}

/// Whether `uri` (a path, with or without a query) is one Ortus serves,
/// with apps at `app_paths`.
pub fn is_served(uri: &str, app_paths: &[String]) -> bool {
    let path = uri.split(['?', '#']).next().unwrap_or_default();
    // No way to climb out of an allowed prefix, written plainly or encoded.
    let lowered = path.to_ascii_lowercase();
    if ["..", "%2e", "%2f", "%5c", "\\", "//", "%00"]
        .iter()
        .any(|trick| lowered.contains(trick))
    {
        return false;
    }
    let under = |base: &str| path == base || path.starts_with(&format!("{base}/"));
    SCREENS.contains(&path)
        || SCREENS.iter().skip(1).any(|screen| under(screen))
        || KNOWN_APP_PATHS.iter().any(|app| under(app))
        || app_paths.iter().any(|app| under(app))
        || PROBES.contains(&path)
        || path == "/.well-known/openid-configuration"
        || crate::app_links::PATHS.contains(&path)
        || (path.starts_with("/auth/saml/")
            && (path.ends_with("/start") || path.ends_with("/acs") || path.ends_with("/metadata"))
            && path.matches('/').count() == 4)
        || (path.starts_with("/auth/external/")
            && (path.ends_with("/start") || path.ends_with("/callback"))
            && path.matches('/').count() == 4)
        || [
            "/oauth2/authorize",
            "/oauth2/token",
            "/oauth2/userinfo",
            "/oauth2/jwks",
        ]
        .contains(&path)
        || path == "/favicon.svg"
        || path.starts_with("/assets/")
        || path == crate::api::PREFIX
        || path.starts_with("/api/")
}

impl Middleware for OnlyOrtusPaths {
    fn handle(
        &self,
        request: &Request,
        connection: &ConnectionInfo,
        next: &dyn Application,
    ) -> Result<Response, String> {
        let path = request
            .request_uri
            .split(['?', '#'])
            .next()
            .unwrap_or_default();
        let lowered = path.to_ascii_lowercase();
        let tricks = ["..", "%2e", "%2f", "%5c", "\\", "//", "%00"]
            .iter()
            .any(|t| lowered.contains(t));
        if is_served(&request.request_uri, &self.app_paths)
            || (!tricks && crate::dav::served(path, &self.dav_ids, &self.well_known))
        {
            next.execute(request, connection)
        } else {
            Ok(not_found())
        }
    }
}
