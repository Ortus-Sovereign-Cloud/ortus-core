//! Who is asking: accounts, sessions and the checks every API route gets.
//!
//! A browser holds its session in an HTTP-only cookie; the phone apps send
//! the same token as `Authorization: Bearer <token>`. Every request that
//! changes something must carry `X-Requested-With: ortus`, which a page on
//! another site cannot add without the browser asking us first (and we
//! never say yes), so no other site can act for a signed-in person.

pub mod accounts;
pub mod sessions;
pub mod setup;

use std::sync::Arc;

use base64::Engine;
use rust_web_server::header::Header;
use rust_web_server::request::Request;

use crate::instance::Instance;
use crate::storage::Schema;
pub use sessions::Session;

/// The schema of accounts, credentials and sessions.
pub const SCHEMA: Schema = Schema {
    app: "core.accounts",
    migrations: accounts::MIGRATIONS,
};

/// Who may call an API route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Anyone, signed in or not.
    Public,
    /// Anyone signed in.
    Signed,
    /// Administrators and owners.
    Admin,
    /// Owners only.
    Owner,
}

/// The name of the session cookie. With HTTPS it carries the `__Host-`
/// prefix, which the browser only accepts from this exact host, over HTTPS,
/// for the whole site.
pub fn cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-ortus"
    } else {
        "ortus"
    }
}

/// `n` random bytes, as lowercase hex.
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("the operating system's random numbers");
    hex::encode(bytes)
}

/// A new secret token: 32 random bytes, URL-safe base64.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random numbers");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The session token a request carries, from its cookie or its
/// `Authorization: Bearer` header.
pub fn token(request: &Request) -> Option<String> {
    if let Some(value) = request.header("authorization") {
        if let Some(token) = value.value.strip_prefix("Bearer ") {
            return Some(token.trim().to_string());
        }
    }
    let cookies = request.header("cookie")?;
    for pair in cookies.value.split(';') {
        let (name, value) = pair.trim().split_once('=')?;
        if name == cookie_name(true) || name == cookie_name(false) {
            return Some(value.to_string());
        }
    }
    None
}

/// The `Set-Cookie` header that keeps `token` in the browser.
pub fn set_cookie(token: &str, secure: bool, max_age_secs: i64) -> Header {
    let flags = if secure { "; Secure" } else { "" };
    Header {
        name: "Set-Cookie".to_string().into(),
        value: format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{flags}",
            cookie_name(secure)
        )
        .into(),
    }
}

/// The `Set-Cookie` header that removes the session cookie.
pub fn clear_cookie(secure: bool) -> Header {
    set_cookie("", secure, 0)
}

/// Whether a request that changes something came from the Ortus client:
/// it must say so in `X-Requested-With`, which other sites cannot send.
pub fn from_our_client(request: &Request) -> bool {
    matches!(request.method.as_str(), "GET" | "HEAD" | "OPTIONS")
        || request
            .header("x-requested-with")
            .is_some_and(|h| h.value.eq_ignore_ascii_case("ortus"))
}

/// The signed-in session of a request, if any.
pub async fn current(request: &Request, ortus: &Arc<Instance>) -> Option<Session> {
    let token = token(request)?;
    if token.is_empty() || token.len() > 128 {
        return None;
    }
    sessions::find(
        ortus.store.as_ref(),
        &token,
        sessions::Lifetime::default(),
        crate::program::now_ms(),
    )
    .await
    .ok()
    .flatten()
}

/// Why a request may not go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Nobody is signed in.
    SignIn,
    /// Signed in, but not allowed.
    Forbidden,
    /// A change that did not come from the Ortus client.
    NotFromClient,
}

/// Checks `access` for a request, returning the session when there is one.
pub async fn check(
    request: &Request,
    ortus: &Arc<Instance>,
    access: Access,
) -> Result<Option<Session>, Refusal> {
    if !from_our_client(request) {
        return Err(Refusal::NotFromClient);
    }
    let session = current(request, ortus).await;
    let needed = match access {
        Access::Public => return Ok(session),
        Access::Signed => accounts::Role::Member,
        Access::Admin => accounts::Role::Admin,
        Access::Owner => accounts::Role::Owner,
    };
    match session {
        None => Err(Refusal::SignIn),
        Some(session) if session.user.role >= needed => Ok(Some(session)),
        Some(_) => Err(Refusal::Forbidden),
    }
}
