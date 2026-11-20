//! The API of passkeys: adding one while signed in, and signing in with
//! one. Passkeys are discoverable: the sign-in page asks for no address.

use std::sync::Arc;

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::accounts::{self, State};
use super::credentials::{self, Kind};
use super::webauthn::{self, b64, unb64, Assertion, RelyingParty};
use super::{limits, Access};
use crate::api::{answer, body, ok, signed_in, ApiError, Method, Route};
use crate::instance::Instance;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, StoreResult};

/// How long the browser has to answer a challenge.
const CHALLENGE_MS: i64 = 5 * 60 * 1000;

fn rp(ortus: &Instance) -> RelyingParty {
    RelyingParty::from_public_url(&ortus.config.public_url)
}

/// The stored id of a passkey: derived from the credential id, so a
/// sign-in finds it directly.
fn row_id(credential_id: &[u8]) -> String {
    format!("pk_{}", hex::encode(Sha256::digest(credential_id)))
}

async fn new_challenge(
    store: &dyn AsyncStore,
    user_id: Option<&str>,
    purpose: &str,
) -> StoreResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random numbers");
    let challenge = b64(&bytes);
    store
        .execute(
            "INSERT INTO core_webauthn_challenges (challenge, user_id, purpose, expires_at) VALUES ($1, $2, $3, $4)",
            &[challenge.as_str().into(), user_id.into(), purpose.into(), (now_ms() + CHALLENGE_MS).into()],
        )
        .await?;
    store
        .execute(
            "DELETE FROM core_webauthn_challenges WHERE expires_at < $1",
            &[now_ms().into()],
        )
        .await?;
    Ok(challenge)
}

/// Takes a challenge (it works once): `Some(user)` when it was issued for
/// `purpose` and is still fresh.
async fn take_challenge(
    store: &dyn AsyncStore,
    challenge: &str,
    purpose: &str,
) -> StoreResult<Option<Option<String>>> {
    let mut tx = store.begin().await?;
    let rows = tx
        .query("SELECT user_id, purpose, expires_at FROM core_webauthn_challenges WHERE challenge = $1", &[challenge.into()])
        .await?;
    tx.execute(
        "DELETE FROM core_webauthn_challenges WHERE challenge = $1",
        &[challenge.into()],
    )
    .await?;
    tx.commit().await?;
    Ok(rows
        .first()
        .filter(|r| r.text(1) == Some(purpose) && r.int(2).unwrap_or_default() > now_ms())
        .map(|r| r.text(0).map(str::to_string)))
}

/// The challenge a browser answered, from its client data.
fn answered_challenge(client_data_json: &[u8]) -> Result<String, ApiError> {
    serde_json::from_slice::<Value>(client_data_json)
        .ok()
        .and_then(|v| v["challenge"].as_str().map(str::to_string))
        .ok_or_else(|| ApiError::bad_request("The passkey answer has no challenge."))
}

fn refused(reason: &str) -> ApiError {
    ApiError {
        status: 401,
        code: "passkey_refused",
        message: format!("The passkey was not accepted: {reason}."),
    }
}

fn decode(field: &str, value: &str) -> Result<Vec<u8>, ApiError> {
    unb64(value).map_err(|_| ApiError::bad_request(format!("{field} is not base64url.")))
}

async fn register_options(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let store = ortus.store.as_ref();
    let challenge = new_challenge(store, Some(&me.user.id), "register").await?;
    let existing: Vec<Value> = credentials::of_kind(store, &me.user.id, &Kind::Passkey)
        .await?
        .iter()
        .map(|c| json!({"type": "public-key", "id": c.data["credential_id"]}))
        .collect();
    let rp = rp(ortus);
    Ok(ok(&json!({
        "challenge": challenge,
        "rp": {"id": rp.id, "name": "Ortus"},
        "user": {"id": b64(me.user.id.as_bytes()), "name": me.user.email, "displayName": me.user.name},
        "pubKeyCredParams": webauthn::ALGORITHMS.iter().map(|alg| json!({"type": "public-key", "alg": alg})).collect::<Vec<_>>(),
        "timeout": CHALLENGE_MS,
        "attestation": "none",
        "authenticatorSelection": {"residentKey": "required", "requireResidentKey": true, "userVerification": "required"},
        "excludeCredentials": existing,
    })))
}

#[derive(Deserialize)]
struct NewPasskey {
    name: Option<String>,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "attestationObject")]
    attestation_object: String,
}

async fn register(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = super::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let input: NewPasskey = body(request)?;
    let client_data = decode("clientDataJSON", &input.client_data_json)?;
    let attestation = decode("attestationObject", &input.attestation_object)?;
    let store = ortus.store.as_ref();
    let challenge = answered_challenge(&client_data)?;
    match take_challenge(store, &challenge, "register").await? {
        Some(Some(user)) if user == me.user.id => {}
        _ => return Err(refused("the request expired; try again")),
    }
    let registered = webauthn::register(&rp(ortus), &challenge, &client_data, &attestation, true)
        .map_err(|e| refused(&e))?;
    let id = row_id(&registered.credential_id);
    let name = input
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Passkey".into());
    let data = json!({
        "credential_id": b64(&registered.credential_id),
        "cose_key": b64(&registered.cose_key),
        "algorithm": registered.algorithm,
        "count": registered.count,
    });
    let inserted = store
        .execute(
            "INSERT INTO core_credentials (id, user_id, kind, name, data, created_at, last_used_at) VALUES ($1, $2, 'passkey', $3, $4, $5, NULL) ON CONFLICT (id) DO NOTHING",
            &[id.as_str().into(), me.user.id.as_str().into(), name.trim().chars().take(100).collect::<String>().into(), data.to_string().into(), now_ms().into()],
        )
        .await?;
    if inserted == 0 {
        return Err(ApiError::conflict("This passkey is already added."));
    }
    super::audit(
        ortus,
        crate::audit::Event::new("auth.credential", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&me.user.id)
            .with(json!({"change": "passkey_added", "passkey": id})),
    )
    .await;
    Ok(ok(&json!({"id": id, "name": name})))
}

async fn sign_in_options(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let _ = request;
    let challenge = new_challenge(ortus.store.as_ref(), None, "sign-in").await?;
    Ok(ok(&json!({
        "challenge": challenge,
        "rpId": rp(ortus).id,
        "timeout": CHALLENGE_MS,
        "userVerification": "required",
        "allowCredentials": [],
    })))
}

#[derive(Deserialize)]
struct PasskeyAnswer {
    id: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
    #[serde(rename = "userHandle")]
    user_handle: Option<String>,
}

async fn sign_in(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let input: PasskeyAnswer = body(request)?;
    let store = ortus.store.as_ref();
    let keys = vec![format!(
        "ip:{}",
        super::client_address(request, connection, &ortus.config.trusted_proxies)
    )];
    for key in &keys {
        if limits::blocked_until(store, key, now_ms()).await?.is_some() {
            return Err(ApiError::too_many("Too many tries. Try again later."));
        }
    }
    let client_data = decode("clientDataJSON", &input.client_data_json)?;
    let authenticator_data = decode("authenticatorData", &input.authenticator_data)?;
    let signature = decode("signature", &input.signature)?;
    let challenge = answered_challenge(&client_data)?;
    if take_challenge(store, &challenge, "sign-in")
        .await?
        .is_none()
    {
        return Err(refused("the request expired; try again"));
    }
    let id = row_id(&decode("id", &input.id)?);
    let rows = store
        .query(
            "SELECT user_id, data FROM core_credentials WHERE id = $1 AND kind = 'passkey'",
            &[id.as_str().into()],
        )
        .await?;
    let failed = |reason: &str| refused(reason);
    let Some(row) = rows.first() else {
        limits::failed(store, &keys, now_ms()).await?;
        return Err(failed("this passkey is not known here"));
    };
    let user_id = row.text(0).unwrap_or_default().to_string();
    let data: Value = serde_json::from_str(row.text(1).unwrap_or("{}")).unwrap_or_default();
    if let Some(handle) = input.user_handle.filter(|h| !h.is_empty()) {
        if decode("userHandle", &handle)? != user_id.as_bytes() {
            return Err(failed("the passkey belongs to another account"));
        }
    }
    let cose = unb64(data["cose_key"].as_str().unwrap_or_default()).unwrap_or_default();
    let stored_count = u32::try_from(data["count"].as_u64().unwrap_or(0)).unwrap_or(0);
    let assertion = Assertion {
        client_data_json: &client_data,
        authenticator_data: &authenticator_data,
        signature: &signature,
    };
    let count = match webauthn::sign_in(
        &rp(ortus),
        &challenge,
        &assertion,
        &cose,
        stored_count,
        true,
    ) {
        Ok(count) => count,
        Err(reason) => {
            limits::failed(store, &keys, now_ms()).await?;
            super::audit(
                ortus,
                crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                    .by(&user_id)
                    .from(&crate::auth::client_address(
                        request,
                        connection,
                        &ortus.config.trusted_proxies,
                    ))
                    .with(json!({"method": "passkey", "reason": reason})),
            )
            .await;
            return Err(failed(&reason));
        }
    };
    let Some(user) = accounts::by_id(store, &user_id)
        .await?
        .filter(|u| u.state == State::Active && u.expires_at.is_none_or(|at| at > now_ms()))
    else {
        return Err(failed("the account cannot sign in"));
    };
    let mut updated = data.clone();
    updated["count"] = json!(count);
    credentials::update(store, &id, &updated, now_ms()).await?;
    limits::succeeded(store, &keys).await?;
    Ok(signed_in(request, connection, ortus, &user, "passkey").await)
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
    let store = ortus.store.as_ref();
    let passkeys = credentials::of_kind(store, &me.user.id, &Kind::Passkey).await?;
    if !passkeys.iter().any(|c| c.id == id) {
        return Err(ApiError::not_found("There is no such passkey."));
    }
    // Never leave an account without a way back in.
    let others = passkeys.len() > 1
        || credentials::has_totp(store, &me.user.id).await?
        || credentials::recovery_codes_left(store, &me.user.id).await? > 0;
    if !others {
        return Err(ApiError::conflict("This is your last way to sign in. Add another passkey, an authenticator app or a recovery kit first."));
    }
    credentials::remove(store, &me.user.id, &id).await?;
    super::audit(
        ortus,
        crate::audit::Event::new("auth.credential", crate::audit::Outcome::Done)
            .by(&me.user.id)
            .on(&me.user.id)
            .with(json!({"change": "passkey_removed", "passkey": id})),
    )
    .await;
    Ok(ok(&json!({"removed": id})))
}

fn h_register_options(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, register_options(&request, &ortus).await) })
}
fn h_register(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, register(&request, &ortus).await) })
}
fn h_sign_in_options(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, sign_in_options(&request, &ortus).await) })
}
fn h_sign_in(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, sign_in(&request, &connection, &ortus).await) })
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

fn route(
    method: Method,
    path: &'static str,
    id: &'static str,
    summary: &'static str,
    access: Access,
    handler: crate::api::Handler,
) -> Route {
    Route {
        method,
        path,
        id,
        tag: "accounts",
        summary,
        access,
        response: any_object,
        handler,
    }
}

pub fn routes() -> Vec<Route> {
    vec![
        route(Method::Post, "/me/passkeys/options", "startMyPasskey", "Options for navigator.credentials.create: a fresh challenge, this installation and the account.", Access::Signed, h_register_options),
        route(Method::Post, "/me/passkeys", "addMyPasskey", "Add the passkey the browser made: {name, clientDataJSON, attestationObject}, base64url.", Access::Signed, h_register),
        route(Method::Delete, "/me/passkeys/:id", "removeMyPasskey", "Remove one of one's passkeys, unless it is the last way to sign in.", Access::Signed, h_remove),
        route(Method::Post, "/session/passkey/options", "startPasskeySignIn", "Options for navigator.credentials.get: a fresh challenge for any passkey of this installation.", Access::Public, h_sign_in_options),
        route(Method::Post, "/session/passkey", "signInWithPasskey", "Sign in with the passkey's answer: {id, clientDataJSON, authenticatorData, signature, userHandle}, base64url.", Access::Public, h_sign_in),
    ]
}
