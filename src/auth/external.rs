//! Signing in through an outside OpenID Connect provider: a national
//! eID service, the organisation's old directory, or any other.
//!
//! An administrator adds the provider (its issuer and the client Ortus is
//! registered as). The sign-in page then offers it: Ortus sends the person
//! there with the code flow and PKCE, takes the code back, and checks the
//! ID token itself: the signature against the provider's published keys
//! (RS256 or ES256), the issuer, the audience, the time and the nonce.
//!
//! The person's Ortus account is the one linked to their subject at that
//! provider; the first time, the one with the address the provider
//! vouches for (when the provider is trusted for that), or a new one
//! (when it may create accounts). Otherwise sign-in is refused.

// rws hands state to handlers in an Arc of its own.
#![allow(clippy::redundant_allocation)]

use std::sync::Arc;

use aws_lc_rs::signature::{
    RsaPublicKeyComponents, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rust_web_server::header::Header;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, NewUser, Role, State, User};
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::form;
use crate::instance::Instance;
use crate::oidc::{challenge_of, sha256_hex};
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "outside sign-in providers",
    sql: "CREATE TABLE core_external_providers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        issuer TEXT NOT NULL,
        client_id TEXT NOT NULL,
        client_secret TEXT,
        scopes TEXT NOT NULL,
        link_by_email BIGINT NOT NULL,
        create_accounts BIGINT NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE core_external_states (
        state_hash TEXT PRIMARY KEY,
        provider TEXT NOT NULL,
        nonce TEXT NOT NULL,
        verifier TEXT NOT NULL,
        return_to TEXT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE TABLE core_external_links (
        provider TEXT NOT NULL,
        subject TEXT NOT NULL,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        created_at BIGINT NOT NULL,
        PRIMARY KEY (provider, subject)
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.external",
    migrations: MIGRATIONS,
};

/// How long the trip to the provider may take.
const STATE_MS: i64 = 10 * 60 * 1000;
/// Clock difference allowed with the provider.
const SKEW_S: i64 = 120;

#[derive(Clone, Debug)]
struct Provider {
    id: String,
    name: String,
    issuer: String,
    client_id: String,
    client_secret: Option<String>,
    scopes: String,
    link_by_email: bool,
    create_accounts: bool,
}

async fn provider(store: &dyn AsyncStore, id: &str) -> StoreResult<Option<Provider>> {
    Ok(store
        .query("SELECT id, name, issuer, client_id, client_secret, scopes, link_by_email, create_accounts FROM core_external_providers WHERE id = $1", &[id.into()])
        .await?
        .first()
        .map(|r| Provider {
            id: r.text(0).unwrap_or_default().into(),
            name: r.text(1).unwrap_or_default().into(),
            issuer: r.text(2).unwrap_or_default().into(),
            client_id: r.text(3).unwrap_or_default().into(),
            client_secret: r.text(4).map(str::to_string),
            scopes: r.text(5).unwrap_or_default().into(),
            link_by_email: r.int(6) == Some(1),
            create_accounts: r.int(7) == Some(1),
        }))
}

async fn get_json(url: &str) -> Result<Value, String> {
    let client = crate::http::client()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("{url}: {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{url}: not JSON: {e}"))
}

/// The provider's discovery document; its issuer must be the one set.
async fn discover(provider: &Provider) -> Result<Value, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        provider.issuer.trim_end_matches('/')
    );
    let document = get_json(&url).await?;
    if document["issuer"].as_str() != Some(provider.issuer.trim_end_matches('/'))
        && document["issuer"].as_str() != Some(provider.issuer.as_str())
    {
        return Err(format!(
            "the provider says its issuer is {}, not {}",
            document["issuer"], provider.issuer
        ));
    }
    Ok(document)
}

fn redirect(location: &str, cookie: Option<Header>) -> Response {
    let mut response = Response::text(STATUS_CODE_REASON_PHRASE.n302_found, "");
    response.headers.push(Header {
        name: "Location".to_string().into(),
        value: location.to_string().into(),
    });
    response.headers.push(Header {
        name: "Cache-Control".to_string().into(),
        value: "no-store".to_string().into(),
    });
    response.headers.extend(cookie);
    response
}

fn failed(reason: &str) -> Response {
    redirect(&format!("/sign-in?external={}", form::encode(reason)), None)
}

fn callback_url(ortus: &Instance, provider: &str) -> String {
    format!(
        "{}/auth/external/{provider}/callback",
        ortus.config.public_url
    )
}

fn start(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let ortus: Arc<Instance> = Arc::clone(&*ortus);
        let id = params.get("id").unwrap_or_default().to_string();
        let Ok(Some(provider)) = provider(ortus.store.as_ref(), &id).await else {
            return failed("unknown");
        };
        let document = match discover(&provider).await {
            Ok(document) => document,
            Err(e) => {
                eprintln!("ortus: sign-in with {id}: {e}");
                return failed("unreachable");
            }
        };
        let Some(authorize) = document["authorization_endpoint"].as_str() else {
            return failed("unreachable");
        };
        let return_to = form::query(&request.request_uri, "return")
            .filter(|r| r.starts_with('/') && !r.starts_with("//") && !r.starts_with("/\\"))
            .unwrap_or_else(|| "/".into());
        let (state, nonce, verifier) = (
            super::random_token(),
            super::random_token(),
            super::random_token(),
        );
        let saved = ortus
            .store
            .execute(
                "INSERT INTO core_external_states (state_hash, provider, nonce, verifier, return_to, expires_at) VALUES ($1, $2, $3, $4, $5, $6)",
                &[sha256_hex(&state).into(), id.as_str().into(), nonce.as_str().into(), verifier.as_str().into(), return_to.into(), (now_ms() + STATE_MS).into()],
            )
            .await;
        if saved.is_err() {
            return failed("server");
        }
        let separator = if authorize.contains('?') { '&' } else { '?' };
        redirect(
            &format!(
                "{authorize}{separator}response_type=code&client_id={}&redirect_uri={}&scope={}&state={state}&nonce={nonce}&code_challenge={}&code_challenge_method=S256",
                form::encode(&provider.client_id),
                form::encode(&callback_url(&ortus, &id)),
                form::encode(&provider.scopes),
                challenge_of(&verifier)
            ),
            None,
        )
    })
}

/// Checks an ID token's signature against the provider's keys and its
/// claims against what was asked; returns the claims.
pub async fn check_id_token(
    token: &str,
    jwks_uri: &str,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    now_s: i64,
) -> Result<Value, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("the ID token is not a JWT".into());
    }
    let decode = |part: &str| {
        URL_SAFE_NO_PAD
            .decode(part.trim_end_matches('='))
            .map_err(|_| "the ID token is not base64url".to_string())
    };
    let header: Value = serde_json::from_slice(&decode(parts[0])?).map_err(|_| "bad JWT header")?;
    let claims: Value = serde_json::from_slice(&decode(parts[1])?).map_err(|_| "bad JWT claims")?;
    let signature = decode(parts[2])?;
    let message = format!("{}.{}", parts[0], parts[1]);
    let keys = get_json(jwks_uri).await?;
    let kid = header["kid"].as_str();
    let candidates: Vec<&Value> = keys["keys"]
        .as_array()
        .map(|k| {
            k.iter()
                .filter(|key| kid.is_none() || key["kid"].as_str() == kid)
                .collect()
        })
        .unwrap_or_default();
    let verified = candidates.iter().any(|key| match header["alg"].as_str() {
        Some("RS256") if key["kty"] == "RSA" => {
            let (Ok(n), Ok(e)) = (
                decode(key["n"].as_str().unwrap_or_default()),
                decode(key["e"].as_str().unwrap_or_default()),
            ) else {
                return false;
            };
            RsaPublicKeyComponents { n: &n, e: &e }
                .verify(&RSA_PKCS1_2048_8192_SHA256, message.as_bytes(), &signature)
                .is_ok()
        }
        Some("ES256") if key["kty"] == "EC" && key["crv"] == "P-256" => {
            let (Ok(x), Ok(y)) = (
                decode(key["x"].as_str().unwrap_or_default()),
                decode(key["y"].as_str().unwrap_or_default()),
            ) else {
                return false;
            };
            let mut point = vec![4u8];
            point.extend(x);
            point.extend(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                .verify(message.as_bytes(), &signature)
                .is_ok()
        }
        _ => false,
    });
    if !verified {
        return Err(format!(
            "the ID token's signature does not match the provider's keys (alg {})",
            header["alg"]
        ));
    }
    if claims["iss"].as_str().map(|i| i.trim_end_matches('/')) != Some(issuer.trim_end_matches('/'))
    {
        return Err("the ID token is from another issuer".into());
    }
    let audience_ok = match &claims["aud"] {
        Value::String(aud) => aud == client_id,
        Value::Array(list) => list.iter().any(|a| a == client_id),
        _ => false,
    };
    if !audience_ok {
        return Err("the ID token is for another client".into());
    }
    if claims["exp"].as_i64().unwrap_or(0) + SKEW_S < now_s {
        return Err("the ID token has expired".into());
    }
    if claims["nonce"].as_str() != Some(nonce) {
        return Err("the ID token's nonce does not match".into());
    }
    if claims["sub"].as_str().is_none_or(str::is_empty) {
        return Err("the ID token names no subject".into());
    }
    Ok(claims)
}

/// The Ortus account for the person the provider vouched for.
async fn account_for(
    store: &dyn AsyncStore,
    provider: &Provider,
    claims: &Value,
) -> Result<User, &'static str> {
    let subject = claims["sub"].as_str().unwrap_or_default();
    let linked = store
        .query(
            "SELECT user_id FROM core_external_links WHERE provider = $1 AND subject = $2",
            &[provider.id.as_str().into(), subject.into()],
        )
        .await
        .map_err(|_| "server")?;
    if let Some(user_id) = linked.first().and_then(|r| r.text(0)) {
        return accounts::by_id(store, user_id)
            .await
            .map_err(|_| "server")?
            .ok_or("no_account");
    }
    let email = claims["email"]
        .as_str()
        .filter(|_| claims["email_verified"] == true || claims["email_verified"] == "true");
    let user = match email {
        Some(email) if provider.link_by_email => accounts::by_email(store, email)
            .await
            .map_err(|_| "server")?,
        _ => None,
    };
    let user = match (user, email) {
        (Some(user), _) => user,
        (None, Some(email)) if provider.create_accounts => {
            let name = claims["name"].as_str().unwrap_or(email);
            accounts::create(
                store,
                &NewUser {
                    email,
                    name,
                    role: Role::Member,
                    state: State::Active,
                    expires_at: None,
                },
                now_ms(),
            )
            .await
            .map_err(|_| "no_account")?
        }
        _ => return Err("no_account"),
    };
    store
        .execute(
            "INSERT INTO core_external_links (provider, subject, user_id, created_at) VALUES ($1, $2, $3, $4) ON CONFLICT (provider, subject) DO NOTHING",
            &[provider.id.as_str().into(), subject.into(), user.id.as_str().into(), now_ms().into()],
        )
        .await
        .map_err(|_| "server")?;
    Ok(user)
}

fn callback(
    request: Request,
    params: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let ortus: Arc<Instance> = Arc::clone(&*ortus);
        let id = params.get("id").unwrap_or_default().to_string();
        let uri = request.request_uri.clone();
        if form::query(&uri, "error").is_some() {
            return failed("cancelled");
        }
        let (code, state) = (
            form::query(&uri, "code").unwrap_or_default(),
            form::query(&uri, "state").unwrap_or_default(),
        );
        let store = ortus.store.as_ref();
        let taken = async {
            let hash = sha256_hex(&state);
            let mut tx = store.begin().await?;
            let rows = tx.query("SELECT provider, nonce, verifier, return_to, expires_at FROM core_external_states WHERE state_hash = $1", &[hash.as_str().into()]).await?;
            tx.execute("DELETE FROM core_external_states WHERE state_hash = $1 OR expires_at < $2", &[hash.as_str().into(), now_ms().into()]).await?;
            tx.commit().await?;
            Ok::<_, crate::storage::StoreError>(rows.into_iter().next())
        }
        .await;
        let Ok(Some(row)) = taken else {
            return failed("expired");
        };
        if row.text(0) != Some(id.as_str()) || row.int(4).unwrap_or_default() < now_ms() {
            return failed("expired");
        }
        let (nonce, verifier, return_to) = (
            row.text(1).unwrap_or_default().to_string(),
            row.text(2).unwrap_or_default().to_string(),
            row.text(3).unwrap_or("/").to_string(),
        );
        let Ok(Some(provider)) = super::external::provider(store, &id).await else {
            return failed("unknown");
        };
        let result = async {
            let document = discover(&provider).await?;
            let token_url = document["token_endpoint"].as_str().ok_or("no token endpoint")?;
            let jwks_uri = document["jwks_uri"].as_str().ok_or("no jwks_uri")?;
            let mut form_body = format!(
                "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
                form::encode(&code),
                form::encode(&callback_url(&ortus, &id)),
                form::encode(&provider.client_id),
                form::encode(&verifier)
            );
            if let Some(secret) = &provider.client_secret {
                form_body.push_str(&format!("&client_secret={}", form::encode(secret)));
            }
            let response = crate::http::client()?
                .post(token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("Accept", "application/json")
                .body(form_body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status();
            let tokens: Value = serde_json::from_slice(&response.bytes().await.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("the token request failed: {status} {tokens}"));
            }
            let id_token = tokens["id_token"].as_str().ok_or("no ID token")?;
            check_id_token(id_token, jwks_uri, &provider.issuer, &provider.client_id, &nonce, now_ms() / 1000).await
        }
        .await;
        let claims = match result {
            Ok(claims) => claims,
            Err(e) => {
                eprintln!("ortus: sign-in with {id}: {e}");
                super::audit(
                    &ortus,
                    crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                        .from(&super::client_address(
                            &request,
                            &connection,
                            &ortus.config.trusted_proxies,
                        ))
                        .with(json!({"method": format!("external:{id}"), "reason": e})),
                )
                .await;
                return failed("refused");
            }
        };
        let user = match account_for(store, &provider, &claims).await {
            Ok(user) if user.state == State::Active => user,
            Ok(_) => return failed("no_account"),
            Err(reason) => return failed(reason),
        };
        match crate::api::start_session(
            &request,
            &connection,
            &ortus,
            &user,
            &format!("external:{id}"),
        )
        .await
        {
            Ok(token) => {
                let lifetime = super::sessions::Lifetime::default();
                redirect(
                    &return_to,
                    Some(super::set_cookie(
                        &token,
                        ortus.config.secure(),
                        lifetime.max_ms / 1000,
                    )),
                )
            }
            Err(_) => failed("server"),
        }
    })
}

/// Registers the trip to and from providers (outside the API prefix).
pub fn register(
    app: rust_web_server::async_state::AsyncAppWithState<Arc<Instance>>,
) -> rust_web_server::async_state::AsyncAppWithState<Arc<Instance>> {
    app.get("/auth/external/:id/start", start)
        .get("/auth/external/:id/callback", callback)
}

async fn list_public(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let rows = ortus
        .store
        .query(
            "SELECT id, name FROM core_external_providers ORDER BY name",
            &[],
        )
        .await?;
    let providers: Vec<Value> = rows.iter().map(|r| json!({"id": r.text(0), "name": r.text(1), "start": format!("/auth/external/{}/start", r.text(0).unwrap_or_default())})).collect();
    Ok(ok(&json!({ "providers": providers })))
}

#[derive(Deserialize)]
struct NewProvider {
    id: String,
    name: String,
    issuer: String,
    client_id: String,
    client_secret: Option<String>,
    scopes: Option<String>,
    link_by_email: bool,
    create_accounts: bool,
}

async fn add(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: NewProvider = body(request)?;
    if input.id.is_empty()
        || input.id.len() > 40
        || !input
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(ApiError::bad_request(
            "The id is lowercase letters, digits and dashes.",
        ));
    }
    let local = input.issuer.starts_with("http://localhost")
        || input.issuer.starts_with("http://127.0.0.1");
    if !(input.issuer.starts_with("https://") || local) {
        return Err(ApiError::bad_request(
            "The issuer must be an https address.",
        ));
    }
    let provider = Provider {
        id: input.id.clone(),
        name: input.name.trim().to_string(),
        issuer: input.issuer.trim_end_matches('/').to_string(),
        client_id: input.client_id,
        client_secret: input.client_secret.filter(|s| !s.is_empty()),
        scopes: input
            .scopes
            .unwrap_or_else(|| "openid email profile".into()),
        link_by_email: input.link_by_email,
        create_accounts: input.create_accounts,
    };
    // Reachable and honest about its issuer before it is offered to anyone.
    discover(&provider)
        .await
        .map_err(|e| ApiError::bad_request(format!("The provider cannot be used: {e}")))?;
    ortus
        .store
        .execute(
            "INSERT INTO core_external_providers (id, name, issuer, client_id, client_secret, scopes, link_by_email, create_accounts, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                provider.id.as_str().into(),
                provider.name.as_str().into(),
                provider.issuer.as_str().into(),
                provider.client_id.as_str().into(),
                provider.client_secret.clone().into(),
                provider.scopes.as_str().into(),
                i64::from(provider.link_by_email).into(),
                i64::from(provider.create_accounts).into(),
                now_ms().into(),
            ],
        )
        .await
        .map_err(|e| match e {
            crate::storage::StoreError::Conflict(_) => ApiError::conflict("A provider with this id exists."),
            _ => ApiError::internal(),
        })?;
    super::audit(
        ortus,
        crate::audit::Event::new("auth.provider_added", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&provider.id)
            .with(json!({"issuer": provider.issuer})),
    )
    .await;
    Ok(ok(
        &json!({ "id": provider.id, "name": provider.name, "redirect_uri": callback_url(ortus, &provider.id) }),
    ))
}

async fn remove(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default().to_string();
    let mut tx = ortus.store.begin().await?;
    tx.execute(
        "DELETE FROM core_external_links WHERE provider = $1",
        &[id.as_str().into()],
    )
    .await?;
    tx.execute(
        "DELETE FROM core_external_states WHERE provider = $1",
        &[id.as_str().into()],
    )
    .await?;
    let removed = tx
        .execute(
            "DELETE FROM core_external_providers WHERE id = $1",
            &[id.as_str().into()],
        )
        .await?;
    tx.commit().await?;
    if removed == 0 {
        return Err(ApiError::not_found("There is no such provider."));
    }
    super::audit(
        ortus,
        crate::audit::Event::new("auth.provider_removed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&id),
    )
    .await;
    Ok(ok(&json!({ "removed": id })))
}

fn h_list(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, list_public(&request, &ortus).await) })
}
fn h_add(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, add(&request, &ortus).await) })
}
fn h_remove(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, remove(&request, &params, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Get, path: "/session/providers", id: "listSignInProviders", tag: "sign-in", summary: "The outside providers people can sign in with, and where each sign-in starts.", access: Access::Public, response: any_object, handler: h_list },
        Route { method: Method::Post, path: "/admin/sign-in/providers", id: "addSignInProvider", tag: "sign-in", summary: "Add an outside OpenID Connect provider: {id, name, issuer, client_id, client_secret, scopes, link_by_email, create_accounts}. Answers the redirect_uri to register at the provider.", access: Access::Owner, response: any_object, handler: h_add },
        Route { method: Method::Delete, path: "/admin/sign-in/providers/:id", id: "removeSignInProvider", tag: "sign-in", summary: "Remove an outside provider and its links to accounts.", access: Access::Owner, response: any_object, handler: h_remove },
    ]
}
