//! Notifications on phones, on both engines: off until an administrator
//! turns them on; a phone registers and gets a fetch token; a notification
//! becomes a push naming only its id (Apple: the right path, topic, push
//! type and a provider token that verifies; a gone device is forgotten);
//! the phone fetches the text with its token and nothing else; the gateway
//! passes a push on; UnifiedPush posts to the phone's endpoint; quiet
//! hours hold pushes back; a device whose session ended gets nothing.

mod common;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::notify::{Kind, Notice};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

const LIVE: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const VOIP: &str = "f00df00df00df00df00df00df00df00df00df00df00df00df00df00df00df00d";
const GONE: &str = "dead0000000000000000000000000000dead0000000000000000000000000000";

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }
}

struct Fakes {
    apple: Arc<Mutex<Vec<Seen>>>,
    unified: Arc<Mutex<Vec<String>>>,
    unified_port: u16,
    public_key: Vec<u8>,
}

/// One fake Apple push service (HTTP/2 without TLS) and one UnifiedPush
/// endpoint for the whole test binary, and the settings that point at them.
fn fakes() -> &'static Fakes {
    static FAKES: OnceLock<Fakes> = OnceLock::new();
    FAKES.get_or_init(|| {
        let apple = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&apple);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let apple_port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                loop {
                    let (socket, _) = listener.accept().await.unwrap();
                    let seen = Arc::clone(&seen);
                    tokio::spawn(async move {
                        let mut conn = h2::server::handshake(socket).await.unwrap();
                        while let Some(Ok((request, mut respond))) = conn.accept().await {
                            let seen = Arc::clone(&seen);
                            tokio::spawn(async move {
                                let (parts, mut body) = request.into_parts();
                                let mut bytes = Vec::new();
                                while let Some(Ok(chunk)) = body.data().await {
                                    let _ = body.flow_control().release_capacity(chunk.len());
                                    bytes.extend_from_slice(&chunk);
                                }
                                let path = parts.uri.path().to_string();
                                seen.lock().unwrap().push(Seen {
                                    path: path.clone(),
                                    headers: parts
                                        .headers
                                        .iter()
                                        .map(|(k, v)| {
                                            (k.to_string(), v.to_str().unwrap_or("").to_string())
                                        })
                                        .collect(),
                                    body: String::from_utf8_lossy(&bytes).into(),
                                });
                                let (status, text) = if path.ends_with(GONE) {
                                    (410, r#"{"reason":"Unregistered"}"#)
                                } else {
                                    (200, "")
                                };
                                let response =
                                    http::Response::builder().status(status).body(()).unwrap();
                                let mut send =
                                    respond.send_response(response, text.is_empty()).unwrap();
                                if !text.is_empty() {
                                    send.send_data(
                                        bytes::Bytes::from_static(text.as_bytes()),
                                        true,
                                    )
                                    .unwrap();
                                }
                            });
                        }
                    });
                }
            });
        });

        let unified = Arc::new(Mutex::new(Vec::new()));
        let got = Arc::clone(&unified);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unified_port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if body.len() >= len {
                            got.lock().unwrap().push(body.to_string());
                            break;
                        }
                    }
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 201 Created\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
            }
        });

        // The publisher's key, made for the test.
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .unwrap();
        let pair = aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
        )
        .unwrap();
        use aws_lc_rs::signature::KeyPair;
        let public_key = pair.public_key().as_ref().to_vec();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );
        let file = std::env::temp_dir().join(format!("ortus-apns-{}.p8", std::process::id()));
        std::fs::write(&file, pem).unwrap();
        std::env::set_var("ORTUS_APNS_KEY_FILE", &file);
        std::env::set_var("ORTUS_APNS_KEY_ID", "KEY1234567");
        std::env::set_var("ORTUS_APNS_TEAM_ID", "TEAM123456");
        std::env::set_var("ORTUS_APNS_TOPIC", "cc.ortus.app");
        std::env::set_var("ORTUS_APNS_URL", format!("http://127.0.0.1:{apple_port}"));
        std::env::set_var("ORTUS_PUSH_ALLOW_PRIVATE", "1");
        Fakes {
            apple,
            unified,
            unified_port,
            public_key,
        }
    })
}

fn call(
    client: &TestClient<impl rust_web_server::application::Application>,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (i16, Value) {
    let url = format!("/api/v1{path}");
    let r = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        _ => client.delete(&url),
    };
    let r = r
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus");
    let r = match body {
        Some(b) => r.body_text(&b.to_string()),
        None => r,
    };
    let out = r.send();
    (
        out.status(),
        serde_json::from_str(out.body_text()).unwrap_or(Value::Null),
    )
}

fn user_id(store: &dyn AsyncStore, email: &str) -> String {
    block_on(ortus_core::auth::accounts::by_email(store, email))
        .unwrap()
        .unwrap()
        .id
}

fn notice(store: &dyn AsyncStore, user: &str, title: &str) -> String {
    notice_in(store, user, title, "chat", "/chat")
}

fn notice_in(store: &dyn AsyncStore, user: &str, title: &str, app: &str, path: &str) -> String {
    block_on(ortus_core::notify::add(
        store,
        user,
        &Notice {
            app,
            kind: Kind::Mention,
            title,
            detail: "now",
            path,
            key: None,
            args: json!({}),
        },
    ))
    .unwrap();
    let rows = block_on(store.query(
        "SELECT id FROM core_notifications WHERE user_id = $1 AND title = $2",
        &[user.into(), title.into()],
    ))
    .unwrap();
    rows[0].text(0).unwrap().to_string()
}

/// Runs the queued push job of a notification (as the worker would).
fn run_push(instance: &ortus_core::instance::Instance, notification: &str) {
    let rows = block_on(instance.store.query(
        "SELECT payload FROM ortus_jobs WHERE id = $1 AND kind = 'core.push'",
        &[format!("push-{notification}").into()],
    ))
    .unwrap();
    let payload = rows
        .first()
        .and_then(|r| r.text(0))
        .expect("a push job")
        .to_string();
    block_on(ortus_core::push::send_job(instance, &payload)).unwrap();
}

fn devices(store: &dyn AsyncStore) -> Vec<String> {
    block_on(store.query("SELECT token FROM core_push_devices ORDER BY token", &[]))
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

fn push(store: Arc<dyn AsyncStore>) {
    let fakes = fakes();
    let instance = testing_store::instance(store, Vec::new());
    let s = instance.store.as_ref();
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let admin = signed_in(&instance, "admin@ministry.example", Role::Admin);
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let _taras = signed_in(&instance, "taras@ministry.example", Role::Member);
    let olena_id = user_id(s, "olena@ministry.example");
    let taras_id = user_id(s, "taras@ministry.example");
    let apple = |token: &str, env: &str| json!({"platform": "apns", "token": token, "environment": env, "utc_offset": 60});

    // Off until an administrator turns it on.
    assert_eq!(
        409,
        call(
            &client,
            &olena,
            "POST",
            "/me/push-devices",
            Some(apple(LIVE, "production"))
        )
        .0
    );
    assert_eq!(
        403,
        call(
            &client,
            &olena,
            "PUT",
            "/admin/push",
            Some(json!({"enabled": true}))
        )
        .0
    );
    let (status, set) = call(
        &client,
        &admin,
        "PUT",
        "/admin/push",
        Some(json!({"enabled": true})),
    );
    assert_eq!(200, status, "{set}");
    assert_eq!(true, set["enabled"]);
    assert_eq!(true, set["own_apple_key"]);
    assert_eq!(ortus_core::push::DEFAULT_GATEWAY, set["gateway"]);

    // Registering: only real tokens and endpoints.
    assert_eq!(
        400,
        call(
            &client,
            &olena,
            "POST",
            "/me/push-devices",
            Some(apple("not-hex", "production"))
        )
        .0
    );
    assert_eq!(
        400,
        call(
            &client,
            &olena,
            "POST",
            "/me/push-devices",
            Some(apple(LIVE, "staging"))
        )
        .0
    );
    assert_eq!(
        400,
        call(
            &client,
            &olena,
            "POST",
            "/me/push-devices",
            Some(json!({"platform": "unifiedpush", "token": "ftp://example.org/x"}))
        )
        .0
    );
    let (status, live) = call(
        &client,
        &olena,
        "POST",
        "/me/push-devices",
        Some(apple(LIVE, "production")),
    );
    assert_eq!(200, status, "{live}");
    assert!(live["fetch_token"].as_str().is_some_and(|f| f.len() >= 32));
    assert_eq!(
        200,
        call(
            &client,
            &olena,
            "POST",
            "/me/push-devices",
            Some(apple(GONE, "sandbox"))
        )
        .0
    );
    let endpoint = format!("http://127.0.0.1:{}/up/olena", fakes.unified_port);
    assert_eq!(
        200,
        call(
            &client,
            &olena,
            "POST",
            "/me/push-devices",
            Some(json!({"platform": "unifiedpush", "token": endpoint}))
        )
        .0
    );

    // A notification: one push per phone, naming only its id.
    fakes.apple.lock().unwrap().clear();
    fakes.unified.lock().unwrap().clear();
    let id = notice(s, &olena_id, "Anna mentioned you in #flood-response");
    run_push(&instance, &id);
    let seen = fakes.apple.lock().unwrap().clone();
    assert_eq!(2, seen.len(), "{seen:?}");
    for push in &seen {
        assert!(
            push.path == format!("/3/device/{LIVE}") || push.path == format!("/3/device/{GONE}"),
            "{}",
            push.path
        );
        assert_eq!("cc.ortus.app", push.header("apns-topic"));
        assert_eq!("alert", push.header("apns-push-type"));
        let body: Value = serde_json::from_str(&push.body).unwrap();
        assert_eq!(id, body["n"]);
        assert_eq!(1, body["aps"]["mutable-content"]);
        assert!(
            !push.body.contains("flood"),
            "no content leaves: {}",
            push.body
        );
        // The provider token verifies with the key's public half.
        let jwt = push
            .header("authorization")
            .strip_prefix("bearer ")
            .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: Value = serde_json::from_slice(&b64.decode(parts[0]).unwrap()).unwrap();
        let claims: Value = serde_json::from_slice(&b64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(json!({"alg": "ES256", "kid": "KEY1234567"}), header);
        assert_eq!("TEAM123456", claims["iss"]);
        aws_lc_rs::signature::UnparsedPublicKey::new(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED,
            &fakes.public_key,
        )
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &b64.decode(parts[2]).unwrap(),
        )
        .expect("the provider token verifies");
    }
    let unified = fakes.unified.lock().unwrap().clone();
    assert_eq!(1, unified.len());
    assert!(
        unified[0].contains(&id) && !unified[0].contains("flood"),
        "{}",
        unified[0]
    );
    // Apple said the second phone is gone: it is forgotten.
    let left = devices(s);
    assert!(
        left.contains(&LIVE.to_string()) && !left.contains(&GONE.to_string()),
        "{left:?}"
    );

    // An iPhone that rings: its VoIP token takes calls (PushKit), its
    // ordinary token everything else; UnifiedPush hears it is a call.
    let mut ringing = apple(LIVE, "production");
    ringing["voip"] = json!(VOIP);
    let (status, again) = call(&client, &olena, "POST", "/me/push-devices", Some(ringing));
    assert_eq!(200, status, "{again}");
    let fetch = again["fetch_token"].as_str().unwrap().to_string();
    let mut bad = apple(LIVE, "production");
    bad["voip"] = json!("not-hex");
    assert_eq!(
        400,
        call(&client, &olena, "POST", "/me/push-devices", Some(bad)).0
    );
    fakes.apple.lock().unwrap().clear();
    fakes.unified.lock().unwrap().clear();
    let ring = notice_in(
        s,
        &olena_id,
        "Taras is calling in #flood-response",
        "calls",
        "/calls?room=r1",
    );
    run_push(&instance, &ring);
    let seen = fakes.apple.lock().unwrap().clone();
    assert_eq!(1, seen.len(), "{seen:?}");
    assert_eq!(format!("/3/device/{VOIP}"), seen[0].path);
    assert_eq!("voip", seen[0].header("apns-push-type"));
    assert_eq!("cc.ortus.app.voip", seen[0].header("apns-topic"));
    let body: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(
        json!({"n": ring, "s": body["s"], "c": 1}),
        body,
        "no content, no alert"
    );
    let unified = fakes.unified.lock().unwrap().clone();
    assert!(
        unified.len() == 1 && unified[0].contains("\"c\":1"),
        "{unified:?}"
    );
    fakes.apple.lock().unwrap().clear();
    let plain = notice(s, &olena_id, "Not a call");
    run_push(&instance, &plain);
    let seen = fakes.apple.lock().unwrap().clone();
    assert!(
        seen.len() == 1
            && seen[0].path == format!("/3/device/{LIVE}")
            && seen[0].header("apns-push-type") == "alert",
        "{seen:?}"
    );

    // The phone reads the text with its fetch token, and only its person's.
    let read = |token: &str, id: &str| {
        let r = client
            .get(&format!("/api/v1/push/notices/{id}"))
            .header("X-Ortus-Push", token)
            .send();
        (
            r.status(),
            serde_json::from_str::<Value>(r.body_text()).unwrap_or(Value::Null),
        )
    };
    let (status, text) = read(&fetch, &id);
    assert_eq!(200, status, "{text}");
    assert_eq!("Anna mentioned you in #flood-response", text["title"]);
    assert_eq!(401, read("wrong", &id).0);
    let other = notice(s, &taras_id, "For Taras only");
    assert_eq!(404, read(&fetch, &other).0);
    // The fetch token is not a session.
    assert_eq!(
        401,
        client
            .get("/api/v1/notifications")
            .header("Authorization", &format!("Bearer {fetch}"))
            .header("X-Requested-With", "ortus")
            .send()
            .status()
    );

    // An expired session's phone reads nothing (the row may linger).
    let kept: i64 = block_on(s.query(
        "SELECT expires_at FROM core_sessions WHERE user_id = $1",
        &[olena_id.as_str().into()],
    ))
    .unwrap()[0]
        .int(0)
        .unwrap();
    block_on(s.execute(
        "UPDATE core_sessions SET expires_at = 1 WHERE user_id = $1",
        &[olena_id.as_str().into()],
    ))
    .unwrap();
    assert_eq!(401, read(&fetch, &id).0, "expired");
    block_on(s.execute(
        "UPDATE core_sessions SET expires_at = $1 WHERE user_id = $2",
        &[kept.into(), olena_id.as_str().into()],
    ))
    .unwrap();
    assert_eq!(200, read(&fetch, &id).0);

    // The gateway passes one push on, with nothing of its content.
    fakes.apple.lock().unwrap().clear();
    let r = client
        .post("/api/v1/push/gateway")
        .header("X-Requested-With", "ortus")
        .body_text(&json!({"token": LIVE, "environment": "production", "notification": "nabc123", "server": "https://other.example"}).to_string())
        .send();
    assert_eq!(200, r.status(), "{}", r.body_text());
    assert_eq!(1, fakes.apple.lock().unwrap().len());
    let bad = client
        .post("/api/v1/push/gateway")
        .header("X-Requested-With", "ortus")
        .body_text(&json!({"token": LIVE, "environment": "production", "notification": "<b>hi</b>", "server": "https://other.example"}).to_string())
        .send();
    assert_eq!(400, bad.status());

    // Do not disturb: nothing goes.
    block_on(s.execute("INSERT INTO core_notify_settings (user_id, quiet_from, quiet_to, dnd) VALUES ($1, NULL, NULL, 1)", &[olena_id.as_str().into()])).unwrap();
    fakes.apple.lock().unwrap().clear();
    let quiet = notice(s, &olena_id, "In the night");
    run_push(&instance, &quiet);
    assert!(fakes.apple.lock().unwrap().is_empty());
    block_on(s.execute(
        "DELETE FROM core_notify_settings WHERE user_id = $1",
        &[olena_id.as_str().into()],
    ))
    .unwrap();

    // Her session ends (signed out): the phone is forgotten, nothing goes.
    block_on(s.execute(
        "DELETE FROM core_sessions WHERE user_id = $1",
        &[olena_id.as_str().into()],
    ))
    .unwrap();
    fakes.apple.lock().unwrap().clear();
    let after = notice(s, &olena_id, "After sign-out");
    run_push(&instance, &after);
    assert!(fakes.apple.lock().unwrap().is_empty());
    assert!(devices(s).is_empty(), "{:?}", devices(s));
    assert_eq!(
        401,
        read(&fetch, &id).0,
        "the fetch token ended with the session"
    );
}

#[test]
fn public_addresses_only() {
    use ortus_core::push::public;
    for ip in [
        "127.0.0.1",
        "10.0.0.8",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
        "0.0.0.0",
    ] {
        assert!(!public(ip.parse().unwrap()), "{ip}");
    }
    for ip in ["1.1.1.1", "168.119.17.57", "2a01:4f8::1"] {
        assert!(public(ip.parse().unwrap()), "{ip}");
    }
}

#[test]
fn push_on_sqlite_and_postgres() {
    let fresh = common::sqlite().unwrap();
    push((fresh.open)());
    if let Some(fresh) = common::postgres() {
        push((fresh.open)());
    }
}
