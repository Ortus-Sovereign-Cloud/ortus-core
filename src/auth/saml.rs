//! Signing in through an outside SAML 2.0 identity provider, narrowly:
//! SP-initiated only, the request by HTTP-Redirect (unsigned), the answer
//! by HTTP-POST, one plain assertion (see [`super::saml_verify`]), no
//! single logout. An owner adds the provider with its entity id, its
//! sign-in address and its signing certificate.
//!
//! The trip is bound to the browser with a cookie set at the start. The
//! provider's answer arrives as a cross-site POST, which a `SameSite=Lax`
//! cookie would not ride on, so over HTTPS the cookie is `SameSite=None;
//! Secure` (and Lax on a plain-HTTP local install, where provider and
//! Ortus are the same site).

// rws hands state to handlers in an Arc of its own.
#![allow(clippy::redundant_allocation)]

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rust_web_server::header::Header;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use super::accounts::{self, NewUser, Role, State, User};
use super::saml_verify::{self, format_time};
use super::Access;
use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::form;
use crate::instance::Instance;
use crate::oidc::sha256_hex;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "saml providers",
    sql: "CREATE TABLE core_saml_providers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        entity_id TEXT NOT NULL,
        sso_url TEXT NOT NULL,
        certificate TEXT NOT NULL,
        email_attribute TEXT NOT NULL,
        link_by_email BIGINT NOT NULL,
        create_accounts BIGINT NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE core_saml_requests (
        request_id TEXT PRIMARY KEY,
        provider TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        return_to TEXT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE TABLE core_saml_seen (
        assertion_id TEXT PRIMARY KEY,
        expires_at BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.saml",
    migrations: MIGRATIONS,
};

const REQUEST_MS: i64 = 10 * 60 * 1000;
const SKEW_S: i64 = 120;

#[derive(Clone)]
struct Provider {
    id: String,
    entity_id: String,
    sso_url: String,
    certificate: String,
    email_attribute: String,
    link_by_email: bool,
    create_accounts: bool,
}

async fn provider(store: &dyn AsyncStore, id: &str) -> StoreResult<Option<Provider>> {
    Ok(store
        .query("SELECT id, entity_id, sso_url, certificate, email_attribute, link_by_email, create_accounts FROM core_saml_providers WHERE id = $1", &[id.into()])
        .await?
        .first()
        .map(|r| Provider {
            id: r.text(0).unwrap_or_default().into(),
            entity_id: r.text(1).unwrap_or_default().into(),
            sso_url: r.text(2).unwrap_or_default().into(),
            certificate: r.text(3).unwrap_or_default().into(),
            email_attribute: r.text(4).unwrap_or_default().into(),
            link_by_email: r.int(5) == Some(1),
            create_accounts: r.int(6) == Some(1),
        }))
}

/// Ortus's own entity id and answer address for a provider.
fn sp(ortus: &Instance, id: &str) -> (String, String) {
    let base = format!("{}/auth/saml/{id}", ortus.config.public_url);
    (format!("{base}/metadata"), format!("{base}/acs"))
}

fn cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-ortus-saml"
    } else {
        "ortus-saml"
    }
}

fn binding_cookie(value: &str, secure: bool, max_age: i64) -> Header {
    // The answer comes back as a cross-site POST: SameSite=None is the
    // only way the cookie rides along over HTTPS.
    let flags = if secure {
        "; SameSite=None; Secure"
    } else {
        "; SameSite=Lax"
    };
    Header {
        name: "Set-Cookie".to_string().into(),
        value: format!(
            "{}={value}; Path=/; HttpOnly; Max-Age={max_age}{flags}",
            cookie_name(secure)
        )
        .into(),
    }
}

fn cookie_value(request: &Request, secure: bool) -> Option<String> {
    request.header("cookie")?.value.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == cookie_name(secure)).then(|| value.to_string())
    })
}

fn redirect(location: &str, headers: Vec<Header>) -> Response {
    let mut response = Response::text(STATUS_CODE_REASON_PHRASE.n302_found, "");
    response.headers.push(Header {
        name: "Location".to_string().into(),
        value: location.to_string().into(),
    });
    response.headers.push(Header {
        name: "Cache-Control".to_string().into(),
        value: "no-store".to_string().into(),
    });
    response.headers.extend(headers);
    response
}

fn failed(reason: &str) -> Response {
    redirect(
        &format!("/sign-in?external={}", form::encode(reason)),
        Vec::new(),
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
        let (entity, acs) = sp(&ortus, &id);
        let request_id = format!("_{}", super::random_hex(20));
        let binding = super::random_token();
        let return_to = form::query(&request.request_uri, "return")
            .and_then(|r| form::safe_return(&r))
            .unwrap_or_else(|| "/".into());
        let saved = ortus
            .store
            .execute(
                "INSERT INTO core_saml_requests (request_id, provider, binding_hash, return_to, expires_at) VALUES ($1, $2, $3, $4, $5)",
                &[request_id.as_str().into(), id.as_str().into(), sha256_hex(&binding).into(), return_to.into(), (now_ms() + REQUEST_MS).into()],
            )
            .await;
        if saved.is_err() {
            return failed("server");
        }
        let xml = format!(
            r#"<samlp:AuthnRequest xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol" xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="{request_id}" Version="2.0" IssueInstant="{}" Destination="{}" ProtocolBinding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" AssertionConsumerServiceURL="{}"><saml:Issuer>{}</saml:Issuer><samlp:NameIDPolicy AllowCreate="true"/></samlp:AuthnRequest>"#,
            format_time(now_ms() / 1000),
            xml_escape(&provider.sso_url),
            xml_escape(&acs),
            xml_escape(&entity)
        );
        let deflated = miniz_oxide::deflate::compress_to_vec(xml.as_bytes(), 8);
        let separator = if provider.sso_url.contains('?') {
            '&'
        } else {
            '?'
        };
        let location = format!(
            "{}{separator}SAMLRequest={}",
            provider.sso_url,
            form::encode(&STANDARD.encode(deflated))
        );
        redirect(
            &location,
            vec![binding_cookie(
                &binding,
                ortus.config.secure(),
                REQUEST_MS / 1000,
            )],
        )
    })
}

/// Checks what the verified assertion says against this request.
fn check_claims(
    verified: &saml_verify::Verified,
    provider: &Provider,
    entity: &str,
    acs: &str,
    request_id: &str,
    now_s: i64,
) -> Result<(), String> {
    if verified.issuer != provider.entity_id {
        return Err(format!(
            "the assertion is from {}, not {}",
            verified.issuer, provider.entity_id
        ));
    }
    if verified.response_status != "urn:oasis:names:tc:SAML:2.0:status:Success" {
        return Err(format!(
            "the provider answered {}",
            verified.response_status
        ));
    }
    if verified
        .response_destination
        .as_deref()
        .is_some_and(|d| d != acs)
    {
        return Err("the response is addressed elsewhere".into());
    }
    if verified.recipient.as_deref() != Some(acs) {
        return Err("the assertion is for another recipient".into());
    }
    if !verified.audiences.iter().any(|a| a == entity) {
        return Err("the assertion is for another audience".into());
    }
    if verified.in_response_to.as_deref() != Some(request_id) {
        return Err("the assertion answers another request".into());
    }
    if verified.not_before.is_some_and(|t| t - SKEW_S > now_s) {
        return Err("the assertion is not valid yet".into());
    }
    let until = verified
        .not_on_or_after
        .into_iter()
        .chain(verified.confirmation_not_on_or_after)
        .min();
    match until {
        Some(t) if t + SKEW_S > now_s => {}
        _ => return Err("the assertion has expired (or says no end)".into()),
    }
    if verified.name_id.is_empty() {
        return Err("the assertion names no one".into());
    }
    Ok(())
}

async fn account_for(
    store: &dyn AsyncStore,
    provider: &Provider,
    verified: &saml_verify::Verified,
) -> Result<User, &'static str> {
    let link = format!("saml:{}", provider.id);
    let linked = store
        .query(
            "SELECT user_id FROM core_external_links WHERE provider = $1 AND subject = $2",
            &[link.as_str().into(), verified.name_id.as_str().into()],
        )
        .await
        .map_err(|_| "server")?;
    if let Some(user_id) = linked.first().and_then(|r| r.text(0)) {
        return accounts::by_id(store, user_id)
            .await
            .map_err(|_| "server")?
            .ok_or("no_account");
    }
    let email = verified
        .attributes
        .iter()
        .find(|(name, _)| *name == provider.email_attribute)
        .and_then(|(_, values)| values.first().cloned())
        .or_else(|| accounts::normal_email(&verified.name_id));
    let user = match &email {
        Some(email) if provider.link_by_email => accounts::by_email(store, email)
            .await
            .map_err(|_| "server")?
            // Never an unverified sign-up: its owner has not proved the address.
            .filter(|u| matches!(u.state, accounts::State::Active | accounts::State::Invited)),
        _ => None,
    };
    let user = match (user, email) {
        (Some(user), _) => user,
        (None, Some(email)) if provider.create_accounts => accounts::create(
            store,
            &NewUser {
                email: &email,
                name: &email,
                role: Role::Member,
                state: State::Active,
                expires_at: None,
            },
            now_ms(),
        )
        .await
        .map_err(|_| "no_account")?,
        _ => return Err("no_account"),
    };
    store
        .execute(
            "INSERT INTO core_external_links (provider, subject, user_id, created_at) VALUES ($1, $2, $3, $4) ON CONFLICT (provider, subject) DO NOTHING",
            &[link.as_str().into(), verified.name_id.as_str().into(), user.id.as_str().into(), now_ms().into()],
        )
        .await
        .map_err(|_| "server")?;
    Ok(user)
}

fn acs(
    request: Request,
    params: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let ortus: Arc<Instance> = Arc::clone(&*ortus);
        let id = params.get("id").unwrap_or_default().to_string();
        let store = ortus.store.as_ref();
        let Ok(Some(provider)) = provider(store, &id).await else {
            return failed("unknown");
        };
        let (entity, acs_url) = sp(&ortus, &id);
        let body = String::from_utf8_lossy(request.body.as_bytes()).into_owned();
        if body.len() > 512 * 1024 {
            return failed("refused");
        }
        let Some(encoded) = form::get(&body, "SAMLResponse") else {
            return failed("refused");
        };
        let Ok(bytes) = STANDARD.decode(encoded.replace(char::is_whitespace, "")) else {
            return failed("refused");
        };
        let xml = String::from_utf8_lossy(&bytes).into_owned();
        let address = super::client_address(&request, &connection, &ortus.config.trusted_proxies);
        let refuse = |reason: String| {
            let ortus = Arc::clone(&ortus);
            let address = address.clone();
            let id = id.clone();
            async move {
                eprintln!("ortus: SAML sign-in with {id}: {reason}");
                super::audit(
                    &ortus,
                    crate::audit::Event::new("auth.sign_in", crate::audit::Outcome::Failed)
                        .from(&address)
                        .with(json!({"method": "saml", "reason": reason})),
                )
                .await;
                failed("refused")
            }
        };
        let key = match saml_verify::rsa_key_of_certificate(&provider.certificate) {
            Ok(key) => key,
            Err(e) => return refuse(format!("the configured certificate: {e}")).await,
        };
        let verified = match saml_verify::verify(&xml, &key) {
            Ok(v) => v,
            Err(e) => return refuse(e).await,
        };
        let request_id = verified.in_response_to.clone().unwrap_or_default();
        let taken = async {
            let mut tx = store.begin().await?;
            let rows = tx.query("SELECT provider, binding_hash, return_to, expires_at FROM core_saml_requests WHERE request_id = $1", &[request_id.as_str().into()]).await?;
            tx.execute("DELETE FROM core_saml_requests WHERE request_id = $1 OR expires_at < $2", &[request_id.as_str().into(), now_ms().into()]).await?;
            tx.commit().await?;
            Ok::<_, crate::storage::StoreError>(rows.into_iter().next())
        }
        .await;
        let Ok(Some(row)) = taken else {
            return failed("expired");
        };
        let bound = cookie_value(&request, ortus.config.secure()).map(|v| sha256_hex(&v));
        if row.text(0) != Some(id.as_str())
            || row.int(3).unwrap_or_default() < now_ms()
            || bound.is_none()
            || bound.as_deref() != row.text(1)
        {
            return failed("expired");
        }
        let return_to = row.text(2).unwrap_or("/").to_string();
        if let Err(e) = check_claims(
            &verified,
            &provider,
            &entity,
            &acs_url,
            &request_id,
            now_ms() / 1000,
        ) {
            return refuse(e).await;
        }
        // An assertion is used once.
        let until = verified
            .not_on_or_after
            .into_iter()
            .chain(verified.confirmation_not_on_or_after)
            .min()
            .unwrap_or(now_ms() / 1000)
            * 1000;
        let fresh = store
            .execute("INSERT INTO core_saml_seen (assertion_id, expires_at) VALUES ($1, $2) ON CONFLICT (assertion_id) DO NOTHING", &[verified.assertion_id.as_str().into(), (until + SKEW_S * 1000).into()])
            .await;
        let _ = store
            .execute(
                "DELETE FROM core_saml_seen WHERE expires_at < $1",
                &[now_ms().into()],
            )
            .await;
        if !matches!(fresh, Ok(1)) {
            return refuse("the assertion was used before".into()).await;
        }
        let user = match account_for(store, &provider, &verified).await {
            Ok(user) if user.state == State::Active => user,
            Ok(_) => return failed("no_account"),
            Err(reason) => return failed(reason),
        };
        match crate::api::start_session(&request, &connection, &ortus, &user, &format!("saml:{id}"))
            .await
        {
            Ok(token) => {
                let lifetime = crate::auth::policy::lifetime(ortus.store.as_ref()).await;
                redirect(
                    &return_to,
                    vec![
                        super::set_cookie(&token, ortus.config.secure(), lifetime.max_ms / 1000),
                        binding_cookie("", ortus.config.secure(), 0),
                    ],
                )
            }
            Err(_) => failed("server"),
        }
    })
}

fn metadata(
    _: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Arc<Instance>>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let id = params.get("id").unwrap_or_default().to_string();
        let (entity, acs) = sp(&ortus, &id);
        let xml = format!(
            r#"<?xml version="1.0"?><md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{}"><md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol"><md:AssertionConsumerService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="{}" index="0"/></md:SPSSODescriptor></md:EntityDescriptor>"#,
            xml_escape(&entity),
            xml_escape(&acs)
        );
        let mut response = Response::text(STATUS_CODE_REASON_PHRASE.n200_ok, &xml);
        response.headers.push(Header {
            name: "Content-Type".to_string().into(),
            value: "application/samlmetadata+xml".to_string().into(),
        });
        response
    })
}

/// Registers the trip to and from SAML providers (outside the API).
pub fn register(
    app: rust_web_server::async_state::AsyncAppWithState<Arc<Instance>>,
) -> rust_web_server::async_state::AsyncAppWithState<Arc<Instance>> {
    app.get("/auth/saml/:id/start", start)
        .post("/auth/saml/:id/acs", acs)
        .get("/auth/saml/:id/metadata", metadata)
}

#[derive(Deserialize)]
struct NewProvider {
    id: String,
    name: String,
    entity_id: String,
    sso_url: String,
    certificate: String,
    email_attribute: Option<String>,
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
    if !crate::form::web_address_ok(&input.sso_url) {
        return Err(ApiError::bad_request("The sign-in address must be https."));
    }
    saml_verify::rsa_key_of_certificate(&input.certificate)
        .map_err(|e| ApiError::bad_request(format!("The certificate cannot be used: {e}.")))?;
    let store = ortus.store.as_ref();
    store
        .execute(
            "INSERT INTO core_saml_providers (id, name, entity_id, sso_url, certificate, email_attribute, link_by_email, create_accounts, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                input.id.as_str().into(),
                input.name.trim().into(),
                input.entity_id.trim().into(),
                input.sso_url.trim().into(),
                input.certificate.trim().into(),
                input.email_attribute.unwrap_or_else(|| "email".into()).into(),
                i64::from(input.link_by_email).into(),
                i64::from(input.create_accounts).into(),
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
            .on(&input.id)
            .with(json!({"saml": input.entity_id})),
    )
    .await;
    let (entity, acs) = sp(ortus, &input.id);
    Ok(ok(
        &json!({ "id": input.id, "entity_id": entity, "acs_url": acs, "metadata": entity }),
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
        &[format!("saml:{id}").into()],
    )
    .await?;
    tx.execute(
        "DELETE FROM core_saml_requests WHERE provider = $1",
        &[id.as_str().into()],
    )
    .await?;
    let removed = tx
        .execute(
            "DELETE FROM core_saml_providers WHERE id = $1",
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
        Route { method: Method::Post, path: "/admin/sign-in/saml", id: "addSamlProvider", tag: "sign-in", summary: "Add a SAML identity provider: {id, name, entity_id, sso_url, certificate, email_attribute, link_by_email, create_accounts}. Answers Ortus's entity id and answer address for the provider.", access: Access::Owner, response: any_object, handler: h_add },
        Route { method: Method::Delete, path: "/admin/sign-in/saml/:id", id: "removeSamlProvider", tag: "sign-in", summary: "Remove a SAML identity provider and its links to accounts.", access: Access::Owner, response: any_object, handler: h_remove },
    ]
}
