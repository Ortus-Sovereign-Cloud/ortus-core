//! Outgoing webhooks on SQLite and PostgreSQL: an owner adds a hook (an
//! administrator may not), its secret is shown once; events after it was
//! added are posted as signed JSON, others are not; a refused post is tried
//! again later; a ping says at once whether the other side took it.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use ortus_core::webhooks;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

fn call<A: rust_web_server::application::Application>(
    client: &TestClient<A>,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (i16, Value) {
    let url = format!("/api/v1{path}");
    let mut r = match method {
        "GET" => client.get(&url),
        "DELETE" => client.delete(&url),
        _ => client.post(&url),
    };
    r = r
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus");
    if let Some(b) = body {
        r = r
            .header("Content-Type", "application/json")
            .body_bytes(b.to_string().into_bytes());
    }
    let response = r.send();
    (
        response.status(),
        serde_json::from_slice(response.body_bytes()).unwrap_or(Value::Null),
    )
}

/// A receiving system: answers each post with the next status in line
/// (200 once they run out) and keeps what it got.
struct Receiver {
    port: u16,
    got: Arc<Mutex<Vec<(String, String)>>>,
}

fn receiver(statuses: Vec<u16>) -> Receiver {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let got = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&got);
    std::thread::spawn(move || {
        let mut statuses = statuses.into_iter();
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then the body by its length.
            loop {
                let n = conn.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&data).to_string();
            let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
            kept.lock()
                .unwrap()
                .push((head.to_string(), body.to_string()));
            let status = statuses.next().unwrap_or(200);
            let _ = conn.write_all(
                format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        }
    });
    Receiver { port, got }
}

fn header<'a>(head: &'a str, name: &str) -> &'a str {
    head.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim())
        })
        .unwrap_or_default()
}

fn sign(secret: &str, timestamp: &str, body: &str) -> String {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret.as_bytes());
    let tag = aws_lc_rs::hmac::sign(&key, format!("{timestamp}.{body}").as_bytes());
    format!("sha256={}", hex::encode(tag.as_ref()))
}

fn filed(store: &dyn AsyncStore, n: i64) {
    block_on(ortus_core::audit::record(
        store,
        &ortus_core::audit::Event::new("records.filed", ortus_core::audit::Outcome::Done)
            .on("case-17")
            .with(json!({"record": format!("r{n}")})),
        ortus_core::program::now_ms(),
    ))
    .unwrap();
}

fn hooking(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(Arc::clone(&store), vec![]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "owner@ministry.example", Role::Owner);
    let admin = signed_in(&instance, "admin@ministry.example", Role::Admin);
    let cases = receiver(vec![503]);
    let url = format!("http://127.0.0.1:{}/hook", cases.port);
    // Before the hook: not posted.
    filed(store.as_ref(), 0);

    let body = json!({"name": "Case management system", "url": url, "events": ["record.filed", "approval.done", "made.up"]});
    assert_eq!(
        403,
        call(
            &client,
            &admin,
            "POST",
            "/admin/webhooks",
            Some(body.clone())
        )
        .0
    );
    assert_eq!(
        400,
        call(
            &client,
            &owner,
            "POST",
            "/admin/webhooks",
            Some(json!({"name": "x", "url": "ftp://x", "events": ["record.filed"]}))
        )
        .0
    );
    let (status, made) = call(&client, &owner, "POST", "/admin/webhooks", Some(body));
    assert_eq!(200, status, "{made}");
    let secret = made["secret"].as_str().unwrap().to_string();
    assert_eq!(
        json!(["approval.done", "record.filed"]),
        made["webhooks"][0]["events"]
    );
    let (_, listed) = call(&client, &admin, "GET", "/admin/integrations", None);
    assert!(
        !listed.to_string().contains(&secret),
        "the secret is shown once"
    );
    assert!(listed["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "approval.done"));

    // A filing: the first post is refused (503), tried again later.
    filed(store.as_ref(), 1);
    block_on(webhooks::run(&instance)).unwrap();
    assert_eq!(1, cases.got.lock().unwrap().len());
    let (_, listed) = call(&client, &admin, "GET", "/admin/integrations", None);
    assert_eq!(
        "it answered 503", listed["webhooks"][0]["lastError"],
        "{listed}"
    );
    assert_eq!(1, listed["webhooks"][0]["waiting"]);
    block_on(store.execute("UPDATE core_webhook_deliveries SET next_at = 0", &[])).unwrap();
    block_on(webhooks::run(&instance)).unwrap();
    let got = cases.got.lock().unwrap().clone();
    assert_eq!(2, got.len());
    let (head, posted) = &got[1];
    assert_eq!("record.filed", header(head, "X-Ortus-Event"));
    assert_eq!(
        sign(&secret, header(head, "X-Ortus-Timestamp"), posted),
        header(head, "X-Ortus-Signature")
    );
    let event: Value = serde_json::from_str(posted).unwrap();
    assert_eq!(
        ("record.filed", "case-17", "r1"),
        (
            event["event"].as_str().unwrap(),
            event["target"].as_str().unwrap(),
            event["detail"]["record"].as_str().unwrap()
        )
    );
    // Nothing more to send; a ping is posted at once.
    block_on(webhooks::run(&instance)).unwrap();
    assert_eq!(2, cases.got.lock().unwrap().len());
    let id = made["id"].as_str().unwrap();
    let (_, pinged) = call(
        &client,
        &admin,
        "POST",
        &format!("/admin/webhooks/{id}/ping"),
        None,
    );
    assert_eq!(true, pinged["ok"], "{pinged}");
    assert_eq!(
        "ping",
        header(&cases.got.lock().unwrap()[2].0, "X-Ortus-Event")
    );
    // Removed: nothing more.
    assert_eq!(
        200,
        call(
            &client,
            &owner,
            "DELETE",
            &format!("/admin/webhooks/{id}"),
            None
        )
        .0
    );
    filed(store.as_ref(), 2);
    block_on(webhooks::run(&instance)).unwrap();
    assert_eq!(3, cases.got.lock().unwrap().len());
    let audit = block_on(store.query(
        "SELECT action FROM core_audit WHERE action LIKE 'webhooks.%' ORDER BY seq",
        &[],
    ))
    .unwrap();
    let actions: Vec<&str> = audit.iter().filter_map(|r| r.text(0)).collect();
    assert_eq!(vec!["webhooks.added", "webhooks.removed"], actions);
    // The API reference is served to the signed in.
    let (status, reference) = call(&client, &admin, "GET", "/reference", None);
    assert_eq!(200, status);
    assert_eq!("3.1.0", reference["openapi"]);
}

#[test]
fn webhooks_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    hooking((fresh.open)());
}

#[test]
fn webhooks_on_postgres() {
    if let Some(fresh) = common::postgres() {
        hooking((fresh.open)());
    }
}
