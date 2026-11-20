//! Signing in with a password and a one-time code, or a recovery code; and
//! managing one's own sign-in methods.

use std::sync::Arc;

use ortus_core::auth::credentials::{totp, unbase32};
use ortus_core::instance::{testing, Instance};
use ortus_core::program::now_ms;
use ortus_core::runtime::block_on;
use rust_web_server::application::Application;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

struct Api<A: Application> {
    client: TestClient<A>,
}

impl<A: Application> Api<A> {
    fn post(&self, path: &str, token: Option<&str>, body: Value) -> TestResponse {
        let mut request = self
            .client
            .post(path)
            .header("X-Requested-With", "ortus")
            .header("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        request.body_text(&body.to_string()).send()
    }
    fn put(&self, path: &str, token: &str, body: Value) -> TestResponse {
        self.client
            .put(path)
            .header("X-Requested-With", "ortus")
            .header("Authorization", &format!("Bearer {token}"))
            .body_text(&body.to_string())
            .send()
    }
    fn get(&self, path: &str, token: &str) -> TestResponse {
        self.client
            .get(path)
            .header("Authorization", &format!("Bearer {token}"))
            .send()
    }
}

fn owner(instance: &Arc<Instance>, api: &Api<impl Application>) -> String {
    let link = block_on(ortus_core::auth::setup::prepare(
        instance,
        Some("setup-token"),
        false,
        0,
    ))
    .unwrap()
    .unwrap();
    assert!(link.ends_with("#setup-token"));
    let created = api.post(
        "/api/v1/setup",
        None,
        json!({"token": "setup-token", "email": "taras@ministry.example", "name": "Taras"}),
    );
    json_of(&created)["session_token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn password_and_one_time_code() {
    let instance = testing::instance(Vec::new());
    let api = Api {
        client: TestClient::new(ortus_core::app(Arc::clone(&instance))),
    };
    let token = owner(&instance, &api);

    let short = api.put("/api/v1/me/password", &token, json!({"password": "short"}));
    assert_eq!("password_too_short", json_of(&short)["error"]["code"]);
    assert_eq!(
        200,
        api.put(
            "/api/v1/me/password",
            &token,
            json!({"password": "flood plan 2026 R-12"})
        )
        .status()
    );
    let change = api.put(
        "/api/v1/me/password",
        &token,
        json!({"password": "another long password", "current": "wrong"}),
    );
    assert_eq!("wrong_current_password", json_of(&change)["error"]["code"]);

    // A password alone is not enough.
    let alone = api.post(
        "/api/v1/session/password",
        None,
        json!({"email": "taras@ministry.example", "password": "flood plan 2026 R-12"}),
    );
    assert_eq!(403, alone.status());
    assert_eq!("second_factor_required", json_of(&alone)["error"]["code"]);

    let started = json_of(&api.post("/api/v1/me/totp", Some(&token), json!({})));
    assert!(started["uri"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/"));
    let secret = unbase32(started["secret"].as_str().unwrap()).unwrap();
    let step = now_ms() / 30_000;
    assert_eq!(
        400,
        api.post(
            "/api/v1/me/totp/confirm",
            Some(&token),
            json!({"code": "000000"})
        )
        .status()
    );
    assert_eq!(
        200,
        api.post(
            "/api/v1/me/totp/confirm",
            Some(&token),
            json!({"code": totp(&secret, step)})
        )
        .status()
    );
    assert_eq!(
        true,
        json_of(&api.get("/api/v1/me/security", &token))["totp"]
    );

    let sign_in = || {
        let answer = json_of(&api.post(
            "/api/v1/session/password",
            None,
            json!({"email": "TARAS@ministry.example", "password": "flood plan 2026 R-12"}),
        ));
        assert_eq!("code", answer["next"], "{answer}");
        answer["ticket"].as_str().unwrap().to_string()
    };
    let ticket = sign_in();
    assert_eq!(
        401,
        api.post(
            "/api/v1/session/code",
            None,
            json!({"ticket": ticket, "code": "123456"})
        )
        .status()
    );
    // The ticket is used up by the wrong code.
    assert_eq!(
        "ticket_expired",
        json_of(&api.post(
            "/api/v1/session/code",
            None,
            json!({"ticket": ticket, "code": totp(&secret, step + 1)})
        ))["error"]["code"]
    );
    // The code used to enrol does not work again.
    let ticket = sign_in();
    assert_eq!(
        401,
        api.post(
            "/api/v1/session/code",
            None,
            json!({"ticket": ticket, "code": totp(&secret, step)})
        )
        .status()
    );
    let ticket = sign_in();
    let done = api.post(
        "/api/v1/session/code",
        None,
        json!({"ticket": ticket, "code": totp(&secret, step + 1)}),
    );
    assert_eq!(200, done.status(), "{}", done.body_text());
    let session = json_of(&done)["session_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        "taras@ministry.example",
        json_of(&api.get("/api/v1/me", &session))["email"]
    );
    assert_eq!(
        "password+totp",
        json_of(&api.get("/api/v1/me/security", &session))["session_methods"]
    );
}

#[test]
fn recovery_codes_work_once() {
    let instance = testing::instance(Vec::new());
    let api = Api {
        client: TestClient::new(ortus_core::app(Arc::clone(&instance))),
    };
    let token = owner(&instance, &api);
    let codes =
        json_of(&api.post("/api/v1/me/recovery-kit", Some(&token), json!({})))["codes"].clone();
    assert_eq!(10, codes.as_array().unwrap().len());
    let code = codes[3].as_str().unwrap().to_lowercase();
    let first = api.post(
        "/api/v1/session/recovery",
        None,
        json!({"email": "taras@ministry.example", "code": code}),
    );
    assert_eq!(200, first.status(), "{}", first.body_text());
    assert_eq!(9, json_of(&first)["recovery_codes_left"]);
    assert_eq!(
        401,
        api.post(
            "/api/v1/session/recovery",
            None,
            json!({"email": "taras@ministry.example", "code": code})
        )
        .status()
    );
    // A new kit replaces the old one.
    api.post("/api/v1/me/recovery-kit", Some(&token), json!({}));
    let old = codes[4].as_str().unwrap();
    assert_eq!(
        401,
        api.post(
            "/api/v1/session/recovery",
            None,
            json!({"email": "taras@ministry.example", "code": old})
        )
        .status()
    );
}

#[test]
fn guessing_is_slowed_down() {
    let instance = testing::instance(Vec::new());
    let api = Api {
        client: TestClient::new(ortus_core::app(Arc::clone(&instance))),
    };
    let token = owner(&instance, &api);
    api.put(
        "/api/v1/me/password",
        &token,
        json!({"password": "flood plan 2026 R-12"}),
    );
    for _ in 0..5 {
        let wrong = api.post(
            "/api/v1/session/password",
            None,
            json!({"email": "taras@ministry.example", "password": "guess guess guess"}),
        );
        assert_eq!("wrong_credentials", json_of(&wrong)["error"]["code"]);
    }
    let blocked = api.post(
        "/api/v1/session/password",
        None,
        json!({"email": "taras@ministry.example", "password": "flood plan 2026 R-12"}),
    );
    assert_eq!(429, blocked.status(), "{}", blocked.body_text());
    // An unknown address answers like a wrong password.
    let unknown = api.post(
        "/api/v1/session/password",
        None,
        json!({"email": "nobody@ministry.example", "password": "flood plan 2026 R-12"}),
    );
    assert_eq!(429, unknown.status(), "the network address is blocked too");
}
