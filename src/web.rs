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
use rust_web_server::state::AppWithState;

#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
struct Client;

/// What the page may load and do. Everything comes from this server:
/// no inline script or style, no outside origin, no framing.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'";

/// Paths of the web client that are screens, not files: each answers
/// with the page, and the client shows the screen for the path.
pub const SCREENS: &[&str] = &["/"];

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
    let headers = vec![
        header("Cache-Control", cache),
        header("X-Content-Type-Options", "nosniff"),
        header("Content-Security-Policy", CONTENT_SECURITY_POLICY),
        header("Referrer-Policy", "no-referrer"),
        header("Cross-Origin-Opener-Policy", "same-origin"),
    ];
    let body = Range::get_content_range(embedded.data.into_owned(), mime(path).to_string());
    Some(Response::get_response(
        STATUS_CODE_REASON_PHRASE.n200_ok,
        Some(headers),
        Some(vec![body]),
    ))
}

fn not_found() -> Response {
    Response::text(STATUS_CODE_REASON_PHRASE.n404_not_found, "Not found")
}

fn page(_: &Request, _: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    file("index.html", "no-cache").unwrap_or_else(not_found)
}

fn asset(_: &Request, params: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    let path = params.get("path").unwrap_or_default();
    // Embedded names only: a path with `..` simply has no such file.
    file(
        &format!("assets/{path}"),
        "public, max-age=31536000, immutable",
    )
    .unwrap_or_else(not_found)
}

fn favicon(_: &Request, _: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    file("favicon.svg", "public, max-age=86400").unwrap_or_else(not_found)
}

/// Registers the web client on `app`.
pub fn register(mut app: AppWithState<()>) -> AppWithState<()> {
    for screen in SCREENS {
        app = app.get(screen, page);
    }
    app.get("/assets/*path", asset).get("/favicon.svg", favicon)
}

/// Lets through only the paths Ortus serves, and refuses the rest.
///
/// rws serves any file under the program's working directory when no
/// route matches. For Ortus that directory can hold the database and the
/// configuration, so nothing may reach that fallback: a request must be
/// for a screen, an embedded file, the API or the health probe.
pub struct OnlyOrtusPaths;

/// Whether `uri` (a path, with or without a query) is one Ortus serves.
pub fn is_served(uri: &str) -> bool {
    let path = uri.split(['?', '#']).next().unwrap_or_default();
    // No way to climb out of an allowed prefix, written plainly or encoded.
    let lowered = path.to_ascii_lowercase();
    if ["..", "%2e", "%2f", "%5c", "\\", "//", "%00"]
        .iter()
        .any(|trick| lowered.contains(trick))
    {
        return false;
    }
    SCREENS.contains(&path)
        || path == "/healthz"
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
        if is_served(&request.request_uri) {
            next.execute(request, connection)
        } else {
            Ok(not_found())
        }
    }
}
