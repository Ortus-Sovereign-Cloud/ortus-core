//! Notifications on phones when the app is closed, with nothing of their
//! content leaving the installation.
//!
//! A phone registers itself (`POST /me/push-devices`): Apple's push
//! service (`apns`, a device token) or UnifiedPush (`unifiedpush`, an https
//! endpoint its distributor gave it). It gets a fetch token, kept by the
//! app, that can read only that device's person's notifications. When a
//! notification is added, a job sends each device a push that names only
//! the notification's id and the installation; the phone fetches the text
//! itself (`GET /push/notices/:id` with the fetch token) and shows it, or a
//! generic "something new" when it cannot.
//!
//! Apple sends to the Ortus Workspace app only for the holder of the
//! publisher's key, so an installation hands Apple pushes to a gateway
//! (`push_gateway`, by default the publisher's): the gateway gets the
//! device token, the notification id and the installation's address,
//! never any content. An installation with the key is its own gateway
//! (`ORTUS_APNS_*`). UnifiedPush goes straight to the phone's endpoint.
//!
//! Pushes are off until an administrator turns them on (`PUT /admin/push`).
//! A device goes when its session ends (sign-out, expiry, reset) or the
//! push service says it is gone; quiet hours and do-not-disturb hold
//! pushes back.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{answer, body, ok, ApiError, Method, Route};
use crate::auth::Access;
use crate::instance::Instance;
use crate::oidc::sha256_hex;
use crate::program::now_ms;
use crate::storage::{AsyncStore, BoxFuture, Migration, Schema, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "phones for notifications",
    sql: "CREATE TABLE core_push_devices (
        id TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES core_users (id),
        session_hash TEXT NOT NULL,
        platform TEXT NOT NULL,
        token TEXT NOT NULL,
        environment TEXT,
        fetch_hash TEXT NOT NULL,
        utc_offset BIGINT NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE UNIQUE INDEX core_push_devices_token ON core_push_devices (platform, token);
    CREATE INDEX core_push_devices_user ON core_push_devices (user_id)",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.push",
    migrations: MIGRATIONS,
};

/// The job that sends one notification's pushes.
pub const JOB: &str = "core.push";
/// The publisher's gateway, for installations without Apple's key.
pub const DEFAULT_GATEWAY: &str = "https://ortus.cc/api/v1/push/gateway";
/// Devices one person may have.
const MAX_DEVICES: i64 = 10;
/// How long after it was made a notification can still be fetched.
const FETCHABLE_MS: i64 = 24 * 60 * 60 * 1000;
const ENABLED: &str = "push_enabled";
const GATEWAY: &str = "push_gateway";

pub async fn enabled(store: &dyn AsyncStore) -> StoreResult<bool> {
    Ok(crate::auth::policy::setting(store, ENABLED)
        .await?
        .as_deref()
        == Some("on"))
}

pub async fn gateway(store: &dyn AsyncStore) -> StoreResult<String> {
    Ok(crate::auth::policy::setting(store, GATEWAY)
        .await?
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| DEFAULT_GATEWAY.to_string()))
}

/// Within a notification's transaction: a push job for it, when its
/// person has a phone registered (no network work here).
pub async fn queue_in(
    tx: &mut dyn crate::storage::AsyncTx,
    user: &str,
    notification: &str,
) -> StoreResult<()> {
    let phones = tx
        .query(
            "SELECT 1 FROM core_push_devices WHERE user_id = $1 LIMIT 1",
            &[user.into()],
        )
        .await?;
    if phones.is_empty() {
        return Ok(());
    }
    let payload = json!({ "user": user, "notification": notification }).to_string();
    crate::jobs::enqueue_in(
        tx,
        &crate::jobs::NewJob {
            id: &format!("push-{notification}"),
            kind: JOB,
            payload: &payload,
            run_at: now_ms(),
            max_attempts: 3,
        },
        now_ms(),
    )
    .await?;
    Ok(())
}

/// Whether an https endpoint may be posted to: a host name or address,
/// and (after lookup) only public addresses: the one to connect to.
pub async fn public_endpoint(url: &str) -> Result<(String, SocketAddr), &'static str> {
    // Tests reach a local endpoint (never set on an installation).
    let inside_allowed = std::env::var("ORTUS_PUSH_ALLOW_PRIVATE").is_ok_and(|v| v == "1");
    let parsed = reqwest::Url::parse(url).map_err(|_| "not a URL")?;
    let scheme_ok = parsed.scheme() == "https" || inside_allowed && parsed.scheme() == "http";
    if !scheme_ok || url.len() > 2000 {
        return Err("not an https URL");
    }
    let host = parsed
        .host_str()
        .ok_or("no host")?
        .trim_matches(['[', ']'])
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| "the host does not resolve")?
        .find(|a| inside_allowed || public(a.ip()))
        .ok_or("not a public address")?;
    Ok((host, addr))
}

/// On the public internet (not loopback, private, link-local, shared or
/// documentation), as for outgoing mail.
pub fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && v6.segments()[1] == 0x0db8)
        }
    }
}

#[derive(Deserialize)]
struct Register {
    platform: String,
    token: String,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    utc_offset: i64,
}

fn refused(status: u16, code: &'static str, message: &str) -> ApiError {
    ApiError {
        status,
        code,
        message: message.into(),
    }
}

/// A phone registers for this person's notifications.
async fn register(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let session = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let store = ortus.store.as_ref();
    if !enabled(store).await? {
        return Err(refused(
            409,
            "push_off",
            "Notifications on phones are off on this installation.",
        ));
    }
    let input: Register = body(request)?;
    let token = input.token.trim().to_string();
    let environment = match input.platform.as_str() {
        "apns" => {
            if token.len() < 32
                || token.len() > 200
                || !token.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(ApiError::bad_request(
                    "An Apple device token is hexadecimal.",
                ));
            }
            match input.environment.as_deref() {
                Some(e @ ("sandbox" | "production")) => Some(e.to_string()),
                _ => {
                    return Err(ApiError::bad_request(
                        "environment is sandbox or production.",
                    ))
                }
            }
        }
        "unifiedpush" => {
            public_endpoint(&token).await.map_err(|why| {
                refused(
                    400,
                    "bad_endpoint",
                    &format!("The endpoint cannot be used: {why}."),
                )
            })?;
            None
        }
        _ => return Err(ApiError::bad_request("platform is apns or unifiedpush.")),
    };
    let user = &session.user.id;
    let count = store
        .query(
            "SELECT CAST(COUNT(*) AS BIGINT) FROM core_push_devices WHERE user_id = $1 AND NOT (platform = $2 AND token = $3)",
            &[user.as_str().into(), input.platform.as_str().into(), token.as_str().into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    if count >= MAX_DEVICES {
        return Err(refused(
            409,
            "too_many",
            "Too many phones are registered: remove one first.",
        ));
    }
    let id = format!("pd{}", crate::auth::random_hex(10));
    let fetch = crate::auth::random_hex(24);
    let mut tx = store.begin().await?;
    // The same phone again (a new session, a new person): it starts over.
    tx.execute(
        "DELETE FROM core_push_devices WHERE platform = $1 AND token = $2",
        &[input.platform.as_str().into(), token.as_str().into()],
    )
    .await?;
    tx.execute(
        "INSERT INTO core_push_devices (id, user_id, session_hash, platform, token, environment, fetch_hash, utc_offset, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &[
            id.as_str().into(),
            user.as_str().into(),
            session.token_hash.as_str().into(),
            input.platform.as_str().into(),
            token.as_str().into(),
            environment.into(),
            sha256_hex(&fetch).into(),
            input.utc_offset.clamp(-14 * 60, 14 * 60).into(),
            now_ms().into(),
        ],
    )
    .await?;
    tx.commit().await?;
    Ok(ok(&json!({ "id": id, "fetch_token": fetch })))
}

async fn unregister(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let session = crate::auth::current(request, ortus)
        .await
        .ok_or_else(ApiError::sign_in)?;
    let id = params.get("id").unwrap_or_default();
    ortus
        .store
        .execute(
            "DELETE FROM core_push_devices WHERE id = $1 AND user_id = $2",
            &[id.into(), session.user.id.as_str().into()],
        )
        .await?;
    Ok(ok(&json!({ "removed": id })))
}

/// A phone fetches one of its person's notifications with its fetch token
/// (`X-Ortus-Push`): only that person's, only recent ones.
async fn notice(
    request: &Request,
    params: &PathParams,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let token = request
        .get_header("X-Ortus-Push".to_string())
        .map(|h| h.value.trim().to_string())
        .unwrap_or_default();
    if token.is_empty() || token.len() > 128 {
        return Err(ApiError::sign_in());
    }
    let store = ortus.store.as_ref();
    let device = store
        .query(
            "SELECT d.user_id FROM core_push_devices d JOIN core_sessions s ON s.token_hash = d.session_hash WHERE d.fetch_hash = $1",
            &[sha256_hex(&token).into()],
        )
        .await?;
    let user = device
        .first()
        .and_then(|r| r.text(0))
        .ok_or_else(ApiError::sign_in)?
        .to_string();
    let id = params.get("id").unwrap_or_default();
    let rows = store
        .query(
            "SELECT app, title, detail, path, at FROM core_notifications WHERE id = $1 AND user_id = $2 AND at > $3",
            &[id.into(), user.as_str().into(), (now_ms() - FETCHABLE_MS).into()],
        )
        .await?;
    let r = rows
        .first()
        .ok_or_else(|| ApiError::not_found("There is no such notification."))?;
    Ok(ok(&json!({
        "app": r.text(0), "title": r.text(1), "detail": r.text(2), "path": r.text(3), "at": r.int(4),
    })))
}

async fn settings(request: &Request, ortus: &Arc<Instance>) -> Result<Response, ApiError> {
    let store = ortus.store.as_ref();
    if request.method == "PUT" {
        #[derive(Deserialize)]
        struct Set {
            enabled: bool,
            #[serde(default)]
            gateway: Option<String>,
        }
        let input: Set = body(request)?;
        if let Some(g) = input.gateway.as_deref().filter(|g| !g.is_empty()) {
            if !g.starts_with("https://") {
                return Err(ApiError::bad_request("The gateway is an https address."));
            }
            crate::auth::policy::set_setting(store, GATEWAY, g).await?;
        }
        crate::auth::policy::set_setting(store, ENABLED, if input.enabled { "on" } else { "off" })
            .await?;
        if let Some(me) = crate::auth::current(request, ortus).await {
            crate::auth::audit(
                ortus,
                crate::audit::Event::new("push.changed", crate::audit::Outcome::Done)
                    .by(&me.user.id)
                    .with(json!({ "enabled": input.enabled })),
            )
            .await;
        }
    }
    let devices = store
        .query(
            "SELECT CAST(COUNT(*) AS BIGINT) FROM core_push_devices",
            &[],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    Ok(ok(&json!({
        "enabled": enabled(store).await?,
        "gateway": gateway(store).await?,
        "own_apple_key": Apns::from_env().is_some(),
        "devices": devices,
    })))
}

// ---- Apple's push service (on a gateway) --------------------------------

/// Apple's push service, with the publisher's key: `ORTUS_APNS_KEY_FILE`
/// (the .p8), `ORTUS_APNS_KEY_ID`, `ORTUS_APNS_TEAM_ID`, `ORTUS_APNS_TOPIC`
/// (the app's bundle id, default cc.ortus.app); `ORTUS_APNS_URL` replaces
/// Apple's host (tests).
pub struct Apns {
    key_der: Vec<u8>,
    key_id: String,
    team_id: String,
    topic: String,
    url: Option<String>,
}

impl Apns {
    pub fn from_env() -> Option<Apns> {
        let file = std::env::var("ORTUS_APNS_KEY_FILE")
            .ok()
            .filter(|f| !f.is_empty())?;
        let pem = std::fs::read_to_string(file).ok()?;
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        use base64::Engine;
        let key_der = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()?;
        Some(Apns {
            key_der,
            key_id: std::env::var("ORTUS_APNS_KEY_ID").ok()?,
            team_id: std::env::var("ORTUS_APNS_TEAM_ID").ok()?,
            topic: std::env::var("ORTUS_APNS_TOPIC").unwrap_or_else(|_| "cc.ortus.app".into()),
            url: std::env::var("ORTUS_APNS_URL")
                .ok()
                .filter(|u| !u.is_empty()),
        })
    }

    /// The provider token (ES256 JWT), made again every 40 minutes (Apple
    /// takes one from 20 to 60 minutes old).
    fn jwt(&self) -> Result<String, String> {
        static CACHE: Mutex<Option<(Instant, String, String)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((made, kid, token)) = cache.as_ref() {
            if made.elapsed() < Duration::from_secs(40 * 60) && *kid == self.key_id {
                return Ok(token.clone());
            }
        }
        use base64::Engine;
        let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
        let header = b64(json!({"alg": "ES256", "kid": self.key_id})
            .to_string()
            .as_bytes());
        let claims = b64(json!({"iss": self.team_id, "iat": now_ms() / 1000})
            .to_string()
            .as_bytes());
        let signing = format!("{header}.{claims}");
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let key = aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &self.key_der,
        )
        .map_err(|_| "the Apple key is not a P-256 PKCS#8 key".to_string())?;
        let sig = key
            .sign(&rng, signing.as_bytes())
            .map_err(|_| "signing failed".to_string())?;
        let token = format!("{signing}.{}", b64(sig.as_ref()));
        *cache = Some((Instant::now(), self.key_id.clone(), token.clone()));
        Ok(token)
    }

    /// Sends one push. `Ok(true)`: the device is gone (forget it).
    pub async fn send(
        &self,
        token: &str,
        environment: &str,
        notification: &str,
        server: &str,
    ) -> Result<bool, String> {
        let host = match (&self.url, environment) {
            (Some(u), _) => u.trim_end_matches('/').to_string(),
            (None, "sandbox") => "https://api.sandbox.push.apple.com".into(),
            (None, _) => "https://api.push.apple.com".into(),
        };
        let client = apns_client(host.starts_with("http://"))?;
        let payload = json!({
            "aps": {"alert": {"loc-key": "push.new"}, "mutable-content": 1, "sound": "default"},
            "n": notification,
            "s": server,
        });
        let r = client
            .post(format!("{host}/3/device/{token}"))
            .header("authorization", format!("bearer {}", self.jwt()?))
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .header(
                "apns-expiration",
                ((now_ms() / 1000) + 24 * 3600).to_string(),
            )
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| format!("Apple's push service: {e}"))?;
        let status = r.status().as_u16();
        let reason = r.text().await.unwrap_or_default();
        match status {
            200 => Ok(false),
            410 => Ok(true),
            400 if reason.contains("BadDeviceToken")
                || reason.contains("DeviceTokenNotForTopic") =>
            {
                Ok(true)
            }
            _ => Err(format!(
                "Apple's push service answered {status}: {}",
                reason.chars().take(200).collect::<String>()
            )),
        }
    }
}

fn apns_client(cleartext: bool) -> Result<reqwest::Client, String> {
    static TLS: std::sync::OnceLock<Result<reqwest::Client, String>> = std::sync::OnceLock::new();
    static PLAIN: std::sync::OnceLock<Result<reqwest::Client, String>> = std::sync::OnceLock::new();
    let make = |plain: bool| {
        let mut b = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .http2_prior_knowledge();
        if !plain {
            let mut tls = crate::http::tls_config(None)?;
            tls.alpn_protocols = vec![b"h2".to_vec()];
            b = b.use_preconfigured_tls(tls);
        }
        b.build().map_err(|e| e.to_string())
    };
    if cleartext {
        PLAIN.get_or_init(|| make(true))
    } else {
        TLS.get_or_init(|| make(false))
    }
    .clone()
}

/// Pushes a gateway takes, per installation address, per minute.
const GATEWAY_PER_MINUTE: u32 = 600;

fn gateway_allows(source: &str) -> bool {
    static SEEN: Mutex<Option<HashMap<String, (Instant, u32)>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = seen.get_or_insert_with(HashMap::new);
    if map.len() > 10_000 {
        map.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(60));
    }
    let entry = map.entry(source.to_string()).or_insert((Instant::now(), 0));
    if entry.0.elapsed() >= Duration::from_secs(60) {
        *entry = (Instant::now(), 0);
    }
    entry.1 += 1;
    entry.1 <= GATEWAY_PER_MINUTE
}

#[derive(Deserialize)]
struct GatewayPush {
    token: String,
    environment: String,
    notification: String,
    server: String,
}

/// The gateway: an installation hands over one Apple push (token, id,
/// its own address; no content). Kept nowhere, logged nowhere.
async fn gateway_push(
    request: &Request,
    connection: &ConnectionInfo,
    ortus: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let Some(apns) = Apns::from_env() else {
        return Err(ApiError::not_found(
            "This installation is not a push gateway.",
        ));
    };
    let source = crate::auth::client_address(request, connection, &ortus.config.trusted_proxies);
    if !gateway_allows(&source) {
        return Err(refused(
            429,
            "too_many",
            "Too many pushes; try again in a minute.",
        ));
    }
    let input: GatewayPush = body(request)?;
    let token_ok = (32..=200).contains(&input.token.len())
        && input.token.bytes().all(|b| b.is_ascii_hexdigit());
    let id_ok = !input.notification.is_empty()
        && input.notification.len() <= 64
        && input
            .notification
            .bytes()
            .all(|b| b.is_ascii_alphanumeric());
    let server_ok = input.server.starts_with("https://") && input.server.len() <= 300
        || input.server.starts_with("http://127.0.0.1")
        || input.server.starts_with("http://localhost");
    if !token_ok
        || !id_ok
        || !server_ok
        || !matches!(input.environment.as_str(), "sandbox" | "production")
    {
        return Err(ApiError::bad_request(
            "A push is {token, environment, notification, server}.",
        ));
    }
    match apns
        .send(
            &input.token,
            &input.environment,
            &input.notification,
            &input.server,
        )
        .await
    {
        Ok(gone) => Ok(ok(&json!({ "gone": gone }))),
        Err(e) => Err(refused(502, "push_failed", &e)),
    }
}

// ---- the job -------------------------------------------------------------

#[derive(Deserialize)]
struct PushJob {
    user: String,
    notification: String,
}

/// Sends one notification to its person's phones.
pub async fn send_job(instance: &Instance, payload: &str) -> Result<(), String> {
    let job: PushJob = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    let store = instance.store.as_ref();
    let fail = |e: crate::storage::StoreError| e.to_string();
    if !enabled(store).await.map_err(fail)? {
        return Ok(());
    }
    // A device whose session ended (sign-out, expiry, a reset) goes now:
    // the phone's next user must not get this one's notifications.
    store
        .execute(
            "DELETE FROM core_push_devices WHERE user_id = $1 AND session_hash NOT IN (SELECT token_hash FROM core_sessions)",
            &[job.user.as_str().into()],
        )
        .await
        .map_err(fail)?;
    let devices = store
        .query(
            "SELECT id, platform, token, environment, utc_offset FROM core_push_devices WHERE user_id = $1",
            &[job.user.as_str().into()],
        )
        .await
        .map_err(fail)?;
    let server = instance.config.public_url.trim_end_matches('/').to_string();
    let gateway_url = gateway(store).await.map_err(fail)?;
    let own = Apns::from_env();
    let mut errors = Vec::new();
    for d in &devices {
        let (id, platform, token) = (
            d.text(0).unwrap_or_default(),
            d.text(1).unwrap_or_default(),
            d.text(2).unwrap_or_default(),
        );
        let offset = d.int(4).unwrap_or(0);
        let minute = ((now_ms() / 60_000 + offset).rem_euclid(1440)) as u32;
        if crate::notify::quiet(store, &job.user, minute)
            .await
            .map_err(fail)?
        {
            continue;
        }
        let gone = match platform {
            "apns" => {
                let env = d.text(3).unwrap_or("production");
                match &own {
                    Some(apns) => apns.send(token, env, &job.notification, &server).await,
                    None => via_gateway(&gateway_url, token, env, &job.notification, &server).await,
                }
            }
            "unifiedpush" => unified_push(token, &job.notification, &server).await,
            _ => Ok(false),
        };
        match gone {
            Ok(true) => {
                let _ = store
                    .execute("DELETE FROM core_push_devices WHERE id = $1", &[id.into()])
                    .await;
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("ortus: push: {e}");
                errors.push(e);
            }
        }
    }
    if errors.is_empty() || errors.len() < devices.len() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

async fn via_gateway(
    gateway: &str,
    token: &str,
    environment: &str,
    notification: &str,
    server: &str,
) -> Result<bool, String> {
    let client = crate::http::client()?;
    let r = client
        .post(gateway)
        .header("X-Requested-With", "ortus")
        .header("Content-Type", "application/json")
        .body(json!({ "token": token, "environment": environment, "notification": notification, "server": server }).to_string())
        .send()
        .await
        .map_err(|e| format!("push gateway: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("push gateway answered {}", r.status()));
    }
    let v: Value = serde_json::from_str(&r.text().await.map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(v["gone"].as_bool().unwrap_or(false))
}

/// UnifiedPush: a POST to the phone's endpoint (a public https address,
/// checked again now and connected to as checked; no redirects).
async fn unified_push(endpoint: &str, notification: &str, server: &str) -> Result<bool, String> {
    let Ok((host, addr)) = public_endpoint(endpoint).await else {
        return Ok(true);
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .use_preconfigured_tls(crate::http::tls_config(None)?)
        .http1_only()
        .build()
        .map_err(|e| e.to_string())?;
    let r = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .body(json!({ "n": notification, "s": server }).to_string())
        .send()
        .await
        .map_err(|e| format!("UnifiedPush endpoint: {e}"))?;
    match r.status().as_u16() {
        200..=299 => Ok(false),
        404 | 410 => Ok(true),
        s => Err(format!("UnifiedPush endpoint answered {s}")),
    }
}

/// The push job, outside any transaction (it waits on the network).
pub fn job(instance: Arc<Instance>) -> crate::app::OutsideJobKind {
    crate::app::OutsideJobKind {
        kind: JOB.into(),
        handler: Box::new(move |job| {
            let instance = Arc::clone(&instance);
            let payload = job.payload.clone();
            Box::pin(async move { send_job(&instance, &payload).await })
        }),
    }
}

// ---- routes --------------------------------------------------------------

fn h_register(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, register(&request, &ortus).await) })
}
fn h_unregister(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, unregister(&request, &params, &ortus).await) })
}
fn h_notice(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, notice(&request, &params, &ortus).await) })
}
fn h_settings(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, settings(&request, &ortus).await) })
}
fn h_gateway(
    request: Request,
    _: PathParams,
    connection: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, gateway_push(&request, &connection, &ortus).await) })
}

fn any_object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let r = |method, path, id, summary, access, handler| Route {
        method,
        path,
        id,
        tag: "notifications",
        summary,
        access,
        response: any_object as fn() -> Value,
        handler,
    };
    vec![
        r(Method::Post, "/me/push-devices", "registerPushDevice", "A phone registers for notifications: {platform (apns or unifiedpush), token (Apple device token or https endpoint), environment (apns: sandbox or production), utc_offset (minutes)}. Answers its id and a fetch token for its notifications.", Access::Signed, h_register as crate::api::Handler),
        r(Method::Delete, "/me/push-devices/:id", "unregisterPushDevice", "A phone stops getting notifications.", Access::Signed, h_unregister),
        r(Method::Get, "/push/notices/:id", "pushNotice", "A phone reads one of its person's recent notifications, with its fetch token in X-Ortus-Push.", Access::Public, h_notice),
        r(Method::Get, "/admin/push", "pushSettings", "Whether notifications go to phones, through which gateway, and how many phones are registered.", Access::Admin, h_settings),
        r(Method::Put, "/admin/push", "setPushSettings", "Turn notifications on phones on or off: {enabled, gateway (https; default the publisher's)}.", Access::Admin, h_settings),
        r(Method::Post, "/push/gateway", "pushGateway", "On a push gateway (an installation with Apple's key): one push for an installation, {token, environment, notification, server}; nothing of the notification's content.", Access::Public, h_gateway),
    ]
}
