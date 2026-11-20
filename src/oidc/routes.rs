//! The endpoints of the OpenID Connect provider, and the admin API for
//! its clients.

// rws hands state to handlers in an Arc of its own, around the Arc the
// rest of Ortus shares.
#![allow(clippy::redundant_allocation)]

use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;
use rust_web_server::header::Header;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{challenge_of, sha256_hex, SigningKey};
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::auth::{self, accounts, Access};
use crate::form;
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::BoxFuture;

const CODE_MS: i64 = 60 * 1000;
const TOKEN_MS: i64 = 60 * 60 * 1000;

fn redirect(location: &str) -> Response {
    let mut response = Response::text(STATUS_CODE_REASON_PHRASE.n302_found, "");
    response.headers.push(Header {
        name: "Location".to_string().into(),
        value: location.to_string().into(),
    });
    response.headers.push(Header {
        name: "Cache-Control".to_string().into(),
        value: "no-store".to_string().into(),
    });
    response
}

/// An error in the OAuth format (RFC 6749 §5.2), not Ortus's own.
fn oauth_error(status: u16, error: &str, description: &str) -> Response {
    let code = match status {
        401 => STATUS_CODE_REASON_PHRASE.n401_unauthorized,
        _ => STATUS_CODE_REASON_PHRASE.n400_bad_request,
    };
    let mut response = Response::json(
        code,
        json!({ "error": error, "error_description": description })
            .to_string()
            .into_bytes(),
    );
    response.headers.push(Header {
        name: "Cache-Control".to_string().into(),
        value: "no-store".to_string().into(),
    });
    response
}

/// A page that says what is wrong with an authorization request that
/// cannot be sent back to the client (an unknown client or redirect).
fn refuse(message: &str) -> Response {
    Response::text(
        STATUS_CODE_REASON_PHRASE.n400_bad_request,
        &format!("This sign-in request is not valid: {message}."),
    )
}

struct Client {
    secret_hash: Option<String>,
    redirect_uris: Vec<String>,
}

async fn client(ortus: &Instance, id: &str) -> Option<Client> {
    let rows = ortus
        .store
        .query(
            "SELECT secret_hash, redirect_uris FROM core_oidc_clients WHERE id = $1",
            &[id.into()],
        )
        .await
        .ok()?;
    let row = rows.first()?;
    Some(Client {
        secret_hash: row.text(0).map(str::to_string),
        redirect_uris: serde_json::from_str(row.text(1).unwrap_or("[]")).unwrap_or_default(),
    })
}

fn discovery(
    _: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let issuer = &ortus.config.public_url;
        ok(&json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/oauth2/authorize"),
            "token_endpoint": format!("{issuer}/oauth2/token"),
            "userinfo_endpoint": format!("{issuer}/oauth2/userinfo"),
            "jwks_uri": format!("{issuer}/oauth2/jwks"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"],
            "scopes_supported": ["openid", "email", "profile"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
            "code_challenge_methods_supported": ["S256"],
            "claims_supported": ["sub", "email", "email_verified", "name", "amr", "auth_time", "nonce"],
        }))
    })
}

fn jwks(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        match SigningKey::current(ortus.store.as_ref(), &ortus.keys, now_ms()).await {
            Ok(key) => ok(&json!({ "keys": [key.jwk()] })),
            Err(_) => ApiError::internal().response(&request),
        }
    })
}

fn authorize(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let ortus: Arc<Instance> = Arc::clone(&*ortus);
        let uri = request.request_uri.clone();
        let q = |name: &str| form::query(&uri, name).unwrap_or_default();
        let (client_id, redirect_uri, state) = (q("client_id"), q("redirect_uri"), q("state"));
        let Some(client) = client(&ortus, &client_id).await else {
            return refuse("the application is not registered here");
        };
        if !client.redirect_uris.contains(&redirect_uri) {
            return refuse("the return address is not one the application registered");
        }
        // From here, errors go back to the application.
        let back = |error: &str| {
            let mut location = format!(
                "{redirect_uri}{}error={error}",
                if redirect_uri.contains('?') { '&' } else { '?' }
            );
            if !state.is_empty() {
                location.push_str(&format!("&state={}", form::encode(&state)));
            }
            redirect(&location)
        };
        if q("response_type") != "code" {
            return back("unsupported_response_type");
        }
        let scope = q("scope");
        if !scope.split(' ').any(|s| s == "openid") {
            return back("invalid_scope");
        }
        let challenge = q("code_challenge");
        if q("code_challenge_method") != "S256" || challenge.len() < 43 {
            return back("invalid_request");
        }
        let Some(session) = auth::current(&request, &ortus).await else {
            // Sign in first, then come back to this same request.
            let path = uri.split('#').next().unwrap_or_default();
            return redirect(&format!("/sign-in?return={}", form::encode(path)));
        };
        let code = auth::random_token();
        let nonce = q("nonce");
        let stored = ortus
            .store
            .execute(
                "INSERT INTO core_oidc_codes (code_hash, client_id, user_id, redirect_uri, challenge, nonce, scope, methods, auth_time, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                &[
                    sha256_hex(&code).into(),
                    client_id.as_str().into(),
                    session.user.id.as_str().into(),
                    redirect_uri.as_str().into(),
                    challenge.into(),
                    (!nonce.is_empty()).then_some(nonce).into(),
                    scope.into(),
                    session.methods.as_str().into(),
                    (session.created_at / 1000).into(),
                    (now_ms() + CODE_MS).into(),
                ],
            )
            .await;
        if stored.is_err() {
            return back("server_error");
        }
        auth::audit(
            &ortus,
            crate::audit::Event::new("oidc.authorize", crate::audit::Outcome::Done)
                .by(&session.user.id)
                .on(&client_id),
        )
        .await;
        let mut location = format!(
            "{redirect_uri}{}code={code}",
            if redirect_uri.contains('?') { '&' } else { '?' }
        );
        if !state.is_empty() {
            location.push_str(&format!("&state={}", form::encode(&state)));
        }
        redirect(&location)
    })
}

/// The client's id and secret, from HTTP Basic or the form.
fn client_credentials(request: &Request, form_text: &str) -> (String, Option<String>) {
    if let Some(value) = request
        .header("authorization")
        .and_then(|h| h.value.strip_prefix("Basic ").map(str::to_string))
    {
        if let Ok(decoded) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, value.trim())
        {
            if let Some((id, secret)) = String::from_utf8_lossy(&decoded).split_once(':') {
                return (form::decode(id), Some(form::decode(secret)));
            }
        }
    }
    (
        form::get(form_text, "client_id").unwrap_or_default(),
        form::get(form_text, "client_secret"),
    )
}

fn token(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let ortus: Arc<Instance> = Arc::clone(&*ortus);
        let text = String::from_utf8_lossy(request.body.as_bytes()).into_owned();
        let field = |name: &str| form::get(&text, name).unwrap_or_default();
        if field("grant_type") != "authorization_code" {
            return oauth_error(
                400,
                "unsupported_grant_type",
                "Only authorization_code is supported.",
            );
        }
        let (client_id, secret) = client_credentials(&request, &text);
        let Some(client) = client(&ortus, &client_id).await else {
            return oauth_error(401, "invalid_client", "Unknown client.");
        };
        if let Some(hash) = &client.secret_hash {
            if secret.as_deref().map(sha256_hex).as_deref() != Some(hash.as_str()) {
                return oauth_error(401, "invalid_client", "Wrong client secret.");
            }
        }
        let code_hash = sha256_hex(&field("code"));
        let store = ortus.store.as_ref();
        let taken = async {
            let mut tx = store.begin().await?;
            let rows = tx
                .query("SELECT client_id, user_id, redirect_uri, challenge, nonce, scope, methods, auth_time, expires_at FROM core_oidc_codes WHERE code_hash = $1", &[code_hash.as_str().into()])
                .await?;
            // A code works once, whatever happens next.
            tx.execute("DELETE FROM core_oidc_codes WHERE code_hash = $1 OR expires_at < $2", &[code_hash.as_str().into(), now_ms().into()]).await?;
            tx.commit().await?;
            Ok::<_, crate::storage::StoreError>(rows.into_iter().next())
        }
        .await;
        let Ok(Some(row)) = taken else {
            return oauth_error(400, "invalid_grant", "The code is not valid.");
        };
        let text_at = |i: usize| row.text(i).unwrap_or_default().to_string();
        if text_at(0) != client_id
            || text_at(2) != field("redirect_uri")
            || row.int(8).unwrap_or_default() < now_ms()
        {
            return oauth_error(
                400,
                "invalid_grant",
                "The code is not valid for this request.",
            );
        }
        if challenge_of(&field("code_verifier")) != text_at(3) {
            return oauth_error(400, "invalid_grant", "The code verifier does not match.");
        }
        let Ok(Some(user)) = accounts::by_id(store, &text_at(1)).await else {
            return oauth_error(400, "invalid_grant", "The account is gone.");
        };
        if user.state != accounts::State::Active {
            return oauth_error(400, "invalid_grant", "The account cannot sign in.");
        }
        let access = auth::random_token();
        let scope = text_at(5);
        if store
            .execute(
                "INSERT INTO core_oidc_tokens (token_hash, client_id, user_id, scope, expires_at) VALUES ($1, $2, $3, $4, $5)",
                &[sha256_hex(&access).into(), client_id.as_str().into(), user.id.as_str().into(), scope.as_str().into(), (now_ms() + TOKEN_MS).into()],
            )
            .await
            .is_err()
        {
            return oauth_error(400, "server_error", "Could not issue a token.");
        }
        let now = now_ms() / 1000;
        let mut claims = json!({
            "iss": ortus.config.public_url,
            "sub": user.id,
            "aud": client_id,
            "iat": now,
            "exp": now + 300,
            "auth_time": row.int(7).unwrap_or(now),
            "amr": text_at(6).split('+').collect::<Vec<_>>(),
        });
        if let Some(nonce) = row.text(4) {
            claims["nonce"] = json!(nonce);
        }
        if scope.split(' ').any(|s| s == "email") {
            claims["email"] = json!(user.email);
            claims["email_verified"] = json!(true);
        }
        if scope.split(' ').any(|s| s == "profile") {
            claims["name"] = json!(user.name);
        }
        let id_token = match SigningKey::current(store, &ortus.keys, now_ms())
            .await
            .map_err(|e| e.to_string())
            .and_then(|key| key.sign(&claims))
        {
            Ok(token) => token,
            Err(_) => return oauth_error(400, "server_error", "Could not sign the ID token."),
        };
        let mut response = ok(&json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": TOKEN_MS / 1000,
            "id_token": id_token,
            "scope": scope,
        }));
        response.headers.push(Header {
            name: "Cache-Control".to_string().into(),
            value: "no-store".to_string().into(),
        });
        response
    })
}

fn userinfo(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let unauthorized = || oauth_error(401, "invalid_token", "A valid access token is needed.");
        let Some(token) = request.header("authorization").and_then(|h| {
            h.value
                .strip_prefix("Bearer ")
                .map(|t| t.trim().to_string())
        }) else {
            return unauthorized();
        };
        let Ok(rows) = ortus
            .store
            .query(
                "SELECT user_id, scope, expires_at FROM core_oidc_tokens WHERE token_hash = $1",
                &[sha256_hex(&token).into()],
            )
            .await
        else {
            return unauthorized();
        };
        let Some(row) = rows
            .first()
            .filter(|r| r.int(2).unwrap_or_default() > now_ms())
        else {
            return unauthorized();
        };
        let Ok(Some(user)) =
            accounts::by_id(ortus.store.as_ref(), row.text(0).unwrap_or_default()).await
        else {
            return unauthorized();
        };
        if user.state != accounts::State::Active {
            return unauthorized();
        }
        let scope = row.text(1).unwrap_or_default();
        let mut info = json!({ "sub": user.id });
        if scope.split(' ').any(|s| s == "email") {
            info["email"] = json!(user.email);
            info["email_verified"] = json!(true);
        }
        if scope.split(' ').any(|s| s == "profile") {
            info["name"] = json!(user.name);
        }
        ok(&info)
    })
}

/// Registers the provider's own endpoints (outside the API prefix).
pub fn register(app: AsyncAppWithState<Arc<Instance>>) -> AsyncAppWithState<Arc<Instance>> {
    app.get("/.well-known/openid-configuration", discovery)
        .get("/oauth2/jwks", jwks)
        .get("/oauth2/authorize", authorize)
        .post("/oauth2/token", token)
        .get("/oauth2/userinfo", userinfo)
}

#[derive(Deserialize)]
struct NewClient {
    name: String,
    redirect_uris: Vec<String>,
    /// A server-side application that keeps a secret; otherwise a public
    /// one (a phone or single-page app) that relies on PKCE alone.
    confidential: bool,
}

async fn add_client(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: NewClient = body(request)?;
    let name = input.name.trim();
    if name.is_empty() || input.redirect_uris.is_empty() {
        return Err(ApiError::bad_request(
            "Give a name and at least one return address.",
        ));
    }
    for uri in &input.redirect_uris {
        let ok = crate::form::redirect_uri_ok(uri);
        if !ok || uri.contains('#') {
            return Err(ApiError::bad_request(format!("{uri} is not a return address Ortus accepts: https, a local address, or an app's own scheme, without #.")));
        }
    }
    let id = format!("ortus-{}", auth::random_hex(10));
    let secret = input.confidential.then(auth::random_token);
    ortus
        .store
        .execute(
            "INSERT INTO core_oidc_clients (id, name, secret_hash, redirect_uris, created_at) VALUES ($1, $2, $3, $4, $5)",
            &[id.as_str().into(), name.into(), secret.as_deref().map(sha256_hex).into(), json!(input.redirect_uris).to_string().into(), now_ms().into()],
        )
        .await?;
    auth::audit(
        ortus,
        crate::audit::Event::new("oidc.client_added", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&id)
            .with(json!({"name": name})),
    )
    .await;
    Ok(ok(
        &json!({ "client_id": id, "client_secret": secret, "name": name, "redirect_uris": input.redirect_uris }),
    ))
}

async fn list_clients(_: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let rows = ortus.store.query("SELECT id, name, secret_hash, redirect_uris, created_at FROM core_oidc_clients ORDER BY name", &[]).await?;
    let clients: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "client_id": r.text(0), "name": r.text(1), "confidential": r.text(2).is_some(),
                "redirect_uris": serde_json::from_str::<Value>(r.text(3).unwrap_or("[]")).unwrap_or_default(), "created_at": r.int(4),
            })
        })
        .collect();
    Ok(ok(&json!({ "clients": clients })))
}

async fn remove_client(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default().to_string();
    let store = ortus.store.as_ref();
    let mut tx = store.begin().await?;
    tx.execute(
        "DELETE FROM core_oidc_tokens WHERE client_id = $1",
        &[id.as_str().into()],
    )
    .await?;
    tx.execute(
        "DELETE FROM core_oidc_codes WHERE client_id = $1",
        &[id.as_str().into()],
    )
    .await?;
    let removed = tx
        .execute(
            "DELETE FROM core_oidc_clients WHERE id = $1",
            &[id.as_str().into()],
        )
        .await?;
    tx.commit().await?;
    if removed == 0 {
        return Err(ApiError::not_found("There is no such application."));
    }
    auth::audit(
        ortus,
        crate::audit::Event::new("oidc.client_removed", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&id),
    )
    .await;
    Ok(ok(&json!({ "removed": id })))
}

fn h_add(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, add_client(&request, &ortus).await) })
}
fn h_list(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, list_clients(&request, &ortus).await) })
}
fn h_remove(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, remove_client(&request, &params, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

/// The admin API for the provider's clients.
pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Post, path: "/admin/oidc/clients", id: "addOidcClient", tag: "sign-in", summary: "Register an application that signs people in with Ortus: {name, redirect_uris, confidential}. The secret is shown only in this answer.", access: Access::Admin, response: any_object, handler: h_add },
        Route { method: Method::Get, path: "/admin/oidc/clients", id: "listOidcClients", tag: "sign-in", summary: "The applications that sign people in with Ortus.", access: Access::Admin, response: any_object, handler: h_list },
        Route { method: Method::Delete, path: "/admin/oidc/clients/:id", id: "removeOidcClient", tag: "sign-in", summary: "Remove an application; its tokens stop working.", access: Access::Admin, response: any_object, handler: h_remove },
    ]
}
