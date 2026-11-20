//! Open sign-up with an email check, through a real mail server (Mailpit,
//! a test tool), and the expiry of demo accounts.
//!
//! Runs when ORTUS_TEST_SMTP (host:port) and ORTUS_TEST_MAILPIT (its API
//! address) are set; CI sets ORTUS_REQUIRE_SMTP=1.

use std::sync::Arc;

use ortus_core::config::Config;
use ortus_core::instance::{testing, Instance};
use ortus_core::program::{now_ms, AppWorker};
use ortus_core::runtime::block_on;
use rust_web_server::application::Application;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};

fn mail_server() -> Option<(String, String)> {
    let Ok(smtp) = std::env::var("ORTUS_TEST_SMTP") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_SMTP").is_err(),
            "ORTUS_REQUIRE_SMTP is set but ORTUS_TEST_SMTP is not"
        );
        eprintln!("skipped: set ORTUS_TEST_SMTP and ORTUS_TEST_MAILPIT");
        return None;
    };
    Some((
        smtp,
        std::env::var("ORTUS_TEST_MAILPIT").expect("ORTUS_TEST_MAILPIT"),
    ))
}

fn demo(smtp: &str, per_address: u32) -> Arc<Instance> {
    let (host, port) = smtp.split_once(':').unwrap();
    let vars = [
        ("ORTUS_SIGNUP_OPEN", "true".to_string()),
        (
            "ORTUS_SIGNUP_ACCOUNTS_PER_ADDRESS_PER_DAY",
            per_address.to_string(),
        ),
        ("ORTUS_MAIL_HOST", host.to_string()),
        ("ORTUS_MAIL_PORT", port.to_string()),
        ("ORTUS_MAIL_SECURITY", "none".to_string()),
        (
            "ORTUS_MAIL_FROM",
            "Ortus demo <noreply@ortus.test>".to_string(),
        ),
        ("ORTUS_PUBLIC_URL", "https://demo.ortus.test".to_string()),
    ];
    let config = Config::load(None, &move |name| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.clone())
    })
    .unwrap();
    testing::instance_with(config, Vec::new())
}

fn post<A: Application>(client: &TestClient<A>, path: &str, body: &Value) -> TestResponse {
    client
        .post(path)
        .header("X-Requested-With", "ortus")
        .body_text(&body.to_string())
        .send()
}

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

/// Runs the worker until the queue is empty.
fn send_queued(instance: &Arc<Instance>) {
    let worker = AppWorker::new(Arc::clone(instance), "test");
    while block_on(worker.tick(now_ms())).unwrap() {}
}

/// The newest message to `to`, as text.
fn last_mail(api: &str, to: &str) -> (String, String) {
    let search = ureq_get(&format!("{api}/api/v1/search?query=to:{to}"));
    let id = search["messages"][0]["ID"]
        .as_str()
        .unwrap_or_else(|| panic!("no mail to {to}: {search}"))
        .to_string();
    let message = ureq_get(&format!("{api}/api/v1/message/{id}"));
    (
        message["Subject"].as_str().unwrap().to_string(),
        message["Text"].as_str().unwrap().to_string(),
    )
}

/// A plain HTTP GET of JSON, with the standard library only.
fn ureq_get(url: &str) -> Value {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    write!(
        stream,
        "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let body = text.split_once("\r\n\r\n").unwrap().1;
    // Mailpit answers chunked; take the JSON between the first { and last }.
    let start = body.find('{').unwrap();
    let end = body.rfind('}').unwrap();
    serde_json::from_str(&body[start..=end]).unwrap()
}

fn code_in(text: &str) -> String {
    text.split_whitespace()
        .find(|w| w.len() == 6 && w.chars().all(|c| c.is_ascii_digit()))
        .unwrap()
        .to_string()
}

#[test]
fn signing_up_checks_the_address_and_makes_a_demo_account() {
    let Some((smtp, api)) = mail_server() else {
        return;
    };
    let instance = demo(&smtp, 3);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let site = json_of(&client.get("/api/v1/site").send());
    assert_eq!(
        (true, 30, 100),
        (
            site["signup_open"].as_bool().unwrap(),
            site["account_days"].as_i64().unwrap(),
            site["quota_mb"].as_i64().unwrap()
        )
    );

    let email = format!("iryna-{}@example.org", now_ms());
    let without_terms = post(
        &client,
        "/api/v1/signup",
        &json!({"email": email, "name": "Iryna", "accept_terms": false}),
    );
    assert_eq!(
        "terms_not_accepted",
        json_of(&without_terms)["error"]["code"]
    );
    let started = post(
        &client,
        "/api/v1/signup",
        &json!({"email": email, "name": "Iryna Savchenko", "language": "uk", "accept_terms": true}),
    );
    assert_eq!(200, started.status(), "{}", started.body_text());
    let ticket = json_of(&started)["ticket"].as_str().unwrap().to_string();
    send_queued(&instance);
    let (subject, text) = last_mail(&api, &email);
    assert!(subject.starts_with("Ваш код Ortus"), "{subject}");
    let code = code_in(&text);

    let wrong = post(
        &client,
        "/api/v1/signup/verify",
        &json!({"ticket": ticket, "code": "000000"}),
    );
    assert_eq!(401, wrong.status());
    let done = post(
        &client,
        "/api/v1/signup/verify",
        &json!({"ticket": ticket, "code": code}),
    );
    assert_eq!(200, done.status(), "{}", done.body_text());
    let body = json_of(&done);
    assert_eq!("add_sign_in_method", body["next"]);
    let expires = body["expires_at"].as_i64().unwrap();
    assert!(
        expires > now_ms() + 29 * 86_400_000,
        "a demo account lasts 30 days"
    );
    // The code works once.
    assert_eq!(
        401,
        post(
            &client,
            "/api/v1/signup/verify",
            &json!({"ticket": ticket, "code": code})
        )
        .status()
    );

    // Signing up again with the address says nothing in the answer, and
    // tells the owner of the address by mail.
    let again = post(
        &client,
        "/api/v1/signup",
        &json!({"email": email, "name": "Someone", "accept_terms": true}),
    );
    assert_eq!(200, again.status());
    send_queued(&instance);
    let (subject, _) = last_mail(&api, &email);
    assert!(
        subject.contains("Ortus") && !subject.contains("код"),
        "{subject}"
    );

    // When its time is up, the expiry job deletes the account.
    let user = block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        &email,
    ))
    .unwrap()
    .unwrap();
    block_on(ortus_core::auth::accounts::set(
        instance.store.as_ref(),
        &user.id,
        ortus_core::auth::accounts::Field::ExpiresAt(Some(1)),
    ))
    .unwrap();
    let job = ortus_core::jobs::NewJob {
        id: "expire-now",
        kind: ortus_core::auth::signup::EXPIRY_JOB,
        payload: "{}",
        run_at: 0,
        max_attempts: 1,
    };
    block_on(ortus_core::jobs::enqueue(instance.store.as_ref(), &job, 0)).unwrap();
    send_queued(&instance);
    assert_eq!(
        None,
        block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            &email
        ))
        .unwrap()
    );
}

#[test]
fn sign_up_is_capped_per_network_address_and_closed_by_default() {
    let Some((smtp, _)) = mail_server() else {
        return;
    };
    let instance = demo(&smtp, 2);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    for n in 0..2 {
        let ok = post(
            &client,
            "/api/v1/signup",
            &json!({"email": format!("cap{n}-{}@example.org", now_ms()), "name": "N", "accept_terms": true}),
        );
        assert_eq!(200, ok.status(), "{}", ok.body_text());
    }
    let third = post(
        &client,
        "/api/v1/signup",
        &json!({"email": format!("cap3-{}@example.org", now_ms()), "name": "N", "accept_terms": true}),
    );
    assert_eq!("signup_address_full", json_of(&third)["error"]["code"]);

    let closed = testing::instance(Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&closed)));
    assert_eq!(
        404,
        post(
            &client,
            "/api/v1/signup",
            &json!({"email": "a@example.org", "name": "A", "accept_terms": true})
        )
        .status()
    );
    assert_eq!(
        false,
        json_of(&client.get("/api/v1/site").send())["signup_open"]
    );
}
