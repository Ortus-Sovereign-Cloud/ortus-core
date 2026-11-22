//! Signing in on one's phone from the web: a person signed in on the web
//! asks for a one-time link (as a QR code); the Ortus app scans or opens
//! it and is signed in as them, then makes the phone their key. The link
//! works once, for five minutes, and only its hash is kept; a newer link
//! replaces an older one.

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
    name: "phone sign-in links",
    sql: "CREATE TABLE core_phone_links (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        created_at BIGINT NOT NULL,
        expires_at BIGINT NOT NULL,
        used_at BIGINT,
        agent TEXT
    );
    CREATE INDEX core_phone_links_user ON core_phone_links (user_id)",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.phone_links",
    migrations: MIGRATIONS,
};

/// How long a link works.
pub const LIFETIME_MS: i64 = 5 * 60 * 1000;

async fn me(request: &Request, ortus: &Arc<Instance>) -> Result<User, ApiError> {
    Ok(super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user)
}

/// The link as a QR code (scanned by the app, or the phone's camera).
fn qr(text: &str) -> Option<String> {
    qrcode::QrCode::new(text.as_bytes()).ok().map(|code| {
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .dark_color(qrcode::render::svg::Color("#0a0b10"))
            .light_color(qrcode::render::svg::Color("#eceef3"))
            .build()
    })
}

/// A new link for the signed-in person (their older ones stop working).
async fn create(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    let store = ortus.store.as_ref();
    let token = super::random_hex(24);
    let now = now_ms();
    let expires_at = now + LIFETIME_MS;
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_phone_links WHERE user_id = $1 OR expires_at < $2",
        &[me.id.as_str().into(), (now - LIFETIME_MS).into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_phone_links (token_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)",
        &[sha256_hex(&token).into(), me.id.as_str().into(), now.into(), expires_at.into()],
    )
    .await?;
    tx.commit().await?;
    let public_url = &ortus.config.public_url;
    // The app's own link when invitations carry one; else the page, which
    // offers the app.
    let link = crate::app_links::app_link_base()
        .map(|base| phone_link(&base, public_url, &token))
        .unwrap_or_else(|| format!("{public_url}/i#{}", fragment(public_url, &token)));
    super::audit(
        ortus,
        crate::audit::Event::new("session.phone_link_created", crate::audit::Outcome::Done)
            .by(&me.id)
            .on(&me.id),
    )
    .await;
    Ok(ok(&json!({
        "link": link,
        "qr_svg": qr(&link),
        "expires_at": expires_at,
    })))
}

fn fragment(public_url: &str, token: &str) -> String {
    let encoded: String = public_url
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("s={encoded}&p={token}")
}

/// The shared app link that signs a phone in: `<base>#s=<installation>&p=<code>`
/// (an invitation's carries `c=` instead).
pub fn phone_link(base: &str, public_url: &str, token: &str) -> String {
    format!("{base}#{}", fragment(public_url, token))
}

/// Whether the person's latest link was used, and by what.
async fn status(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = me(request, ortus).await?;
    let rows = ortus
        .store
        .query(
            "SELECT used_at, agent, expires_at FROM core_phone_links WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1",
            &[me.id.as_str().into()],
        )
        .await?;
    let row = rows.first();
    Ok(ok(&json!({
        "used_at": row.and_then(|r| r.int(0)),
        "agent": row.and_then(|r| r.text(1)),
        "expires_at": row.and_then(|r| r.int(2)),
    })))
}

#[derive(Deserialize)]
struct Use {
    token: String,
}

/// The app follows the link: signed in as the person, once.
async fn follow(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: Use = body(request)?;
    let store = ortus.store.as_ref();
    let hash = sha256_hex(input.token.trim());
    let agent = request
        .get_header("User-Agent".to_string())
        .map(|h| h.value.chars().take(300).collect::<String>())
        .unwrap_or_default();
    let now = now_ms();
    // Used once: only the first to mark it gets in.
    let marked = store
        .execute(
            "UPDATE core_phone_links SET used_at = $2, agent = $3 WHERE token_hash = $1 AND used_at IS NULL AND expires_at >= $2",
            &[hash.as_str().into(), now.into(), agent.as_str().into()],
        )
        .await?;
    let gone = || {
        ApiError {
        status: 410,
        code: "phone_link_expired",
        message: "This sign-in code has expired or was used. Make a new one on the web: your name, then Sign in on your phone.".into(),
    }
    };
    if marked == 0 {
        return Err(gone());
    }
    let rows = store
        .query(
            "SELECT user_id FROM core_phone_links WHERE token_hash = $1",
            &[hash.as_str().into()],
        )
        .await?;
    let user_id = rows
        .first()
        .and_then(|r| r.text(0))
        .ok_or_else(gone)?
        .to_string();
    let user = accounts::by_id(store, &user_id)
        .await?
        .filter(|u| u.state == State::Active)
        .ok_or_else(gone)?;
    super::audit(
        ortus,
        crate::audit::Event::new("session.phone_link_used", crate::audit::Outcome::Done)
            .by(&user.id)
            .on(&user.id),
    )
    .await;
    Ok(crate::api::signed_in_with(
        request,
        connection,
        ortus,
        &user,
        "phone_link",
        json!({ "next": "add_sign_in_method" }),
    )
    .await)
}

fn h_create(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, create(&request, &ortus).await) })
}

fn h_status(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, status(&request, &ortus).await) })
}

fn h_follow(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, follow(&request, &connection, &ortus).await) })
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
        r(Method::Post, "/me/phone-link", "makePhoneLink", "A one-time link (and QR code) that signs the Ortus app in as you: it works once, for five minutes; a newer one replaces it.", Access::Signed, h_create as crate::api::Handler),
        r(Method::Get, "/me/phone-link", "getPhoneLink", "Whether your latest phone link was used (used_at) and by what (agent).", Access::Signed, h_status),
        r(Method::Post, "/session/phone-link", "followPhoneLink", "The app follows a phone link: {token}. Signs in as its person; next, add a sign-in method.", Access::Public, h_follow),
    ]
}
