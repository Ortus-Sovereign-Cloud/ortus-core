//! The phone's way into a web page: the app (signed in) asks for a ticket
//! and opens `/session/web#t=<ticket>&next=<path>` in the system's browser
//! view; the page shows whose ticket it is, and on the person's yes trades
//! it for a web session and goes on to the path. A ticket works once, for
//! a minute, and only its hash is kept. It travels in the address's
//! fragment, so no server log holds it; it is traded by POST. The yes keeps
//! a link from someone else from quietly signing a browser in as them.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, State, User};
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::instance::Instance;
use crate::oidc::sha256_hex;
use crate::program::now_ms;
use crate::storage::{BoxFuture, Migration, Schema};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "tickets from the phone to the web",
    sql: "CREATE TABLE core_web_tickets (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        created_at BIGINT NOT NULL,
        expires_at BIGINT NOT NULL,
        used_at BIGINT
    );
    CREATE INDEX core_web_tickets_user ON core_web_tickets (user_id)",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.web_tickets",
    migrations: MIGRATIONS,
};

/// How long a ticket works.
pub const LIFETIME_MS: i64 = 60 * 1000;

#[derive(Deserialize)]
struct Make {
    /// The page to go on to (a path on this site).
    #[serde(default)]
    next: Option<String>,
}

/// A ticket for the signed-in person, and the address that uses it.
async fn create(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let input: Make = body(request)?;
    let next = input
        .next
        .as_deref()
        .and_then(crate::form::safe_return)
        .unwrap_or_else(|| "/".to_string());
    let store = ortus.store.as_ref();
    let token = super::random_hex(24);
    let now = now_ms();
    let expires_at = now + LIFETIME_MS;
    // The person's spent and old ones go.
    store
        .execute(
            "DELETE FROM core_web_tickets WHERE user_id = $1 AND (used_at IS NOT NULL OR expires_at < $2)",
            &[me.id.as_str().into(), now.into()],
        )
        .await?;
    store
        .execute(
            "INSERT INTO core_web_tickets (token_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)",
            &[sha256_hex(&token).into(), me.id.as_str().into(), now.into(), expires_at.into()],
        )
        .await?;
    let url = format!(
        "{}/session/web#t={token}&next={}",
        ortus.config.public_url,
        encode(&next)
    );
    Ok(ok(&json!({ "url": url, "expires_at": expires_at })))
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
struct Use {
    ticket: String,
}

fn gone() -> ApiError {
    ApiError {
        status: 410,
        code: "web_ticket_expired",
        message: "This link has expired or was used. Open the page from the app again.".into(),
    }
}

/// The ticket's person, if it still works (it is not used up).
async fn holder(ortus: &Arc<Instance>, ticket: &str) -> Result<User, ApiError> {
    let store = ortus.store.as_ref();
    let rows = store
        .query(
            "SELECT user_id FROM core_web_tickets WHERE token_hash = $1 AND used_at IS NULL AND expires_at >= $2",
            &[sha256_hex(ticket.trim()).into(), now_ms().into()],
        )
        .await?;
    let user_id = rows
        .first()
        .and_then(|r| r.text(0))
        .ok_or_else(gone)?
        .to_string();
    accounts::by_id(store, &user_id)
        .await?
        .filter(|u| u.state == State::Active)
        .ok_or_else(gone)
}

/// Whose ticket it is: the page asks the person before using it.
async fn peek(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let input: Use = body(request)?;
    let user = holder(ortus, &input.ticket).await?;
    Ok(ok(&json!({ "name": user.name, "email": user.email })))
}

/// The page trades the ticket for a web session, once.
async fn trade(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: Use = body(request)?;
    let user = holder(ortus, &input.ticket).await?;
    // Used once: only the first to mark it gets in.
    let marked = ortus
        .store
        .execute(
            "UPDATE core_web_tickets SET used_at = $2 WHERE token_hash = $1 AND used_at IS NULL AND expires_at >= $2",
            &[sha256_hex(input.ticket.trim()).into(), now_ms().into()],
        )
        .await?;
    if marked == 0 {
        return Err(gone());
    }
    super::audit(
        ortus,
        crate::audit::Event::new("session.web_ticket_used", crate::audit::Outcome::Done)
            .by(&user.id)
            .on(&user.id),
    )
    .await;
    Ok(
        crate::api::signed_in_with(request, connection, ortus, &user, "web_ticket", json!({}))
            .await,
    )
}

fn h_create(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, create(&request, &ortus).await) })
}

fn h_peek(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, peek(&request, &ortus).await) })
}

fn h_trade(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, trade(&request, &connection, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, access, handler| Route {
        method,
        path,
        id,
        tag: "sessions",
        summary,
        access,
        response: any_object as fn() -> Value,
        handler,
    };
    vec![
        r(Method::Post, "/me/web-ticket", "makeWebTicket", "A ticket that signs the web in as you, once, within a minute: {next} (a path); answers {url} to open.", Access::Signed, h_create as crate::api::Handler),
        r(Method::Post, "/session/web-ticket/whose", "webTicketWhose", "Whose a web ticket is, without using it: {ticket}.", Access::Public, h_peek),
        r(Method::Post, "/session/web-ticket", "useWebTicket", "Trades a web ticket for a web session, once: {ticket}.", Access::Public, h_trade),
    ]
}
