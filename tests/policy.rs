//! The sign-in policy, on both engines: who may set it, that passwords
//! cannot be turned off while someone depends on one, that turning them
//! off refuses password sign-in, and that session lengths follow it.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::application::Application;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

fn send<A: Application>(
    client: &TestClient<A>,
    method: &str,
    path: &str,
    token: &str,
    body: &str,
) -> TestResponse {
    let request = match method {
        "GET" => client.get(path),
        "PUT" => client.put(path),
        _ => client.post(path),
    };
    let request = if token.is_empty() {
        request
    } else {
        request.header("Authorization", &format!("Bearer {token}"))
    };
    request
        .header("X-Requested-With", "ortus")
        .body_text(body)
        .send()
}

fn policy(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let admin = signed_in(&instance, "marek@ministry.example", Role::Admin);
    let member = signed_in(&instance, "ana@ministry.example", Role::Member);
    let ana = block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        "ana@ministry.example",
    ))
    .unwrap()
    .unwrap();
    block_on(ortus_core::auth::credentials::set_password(
        instance.store.as_ref(),
        &ana.id,
        "a long enough passphrase",
        1,
    ))
    .unwrap();
    // An authenticator app beside it: Ana signs in with password and code.
    block_on(instance.store.execute(
        "INSERT INTO core_credentials (id, user_id, kind, name, data, created_at) SELECT 'ana-totp', user_id, 'totp', 'App', 'x', 1 FROM core_credentials WHERE user_id = $1 AND kind = 'password'",
        &[ana.id.as_str().into()],
    ))
    .unwrap();

    assert_eq!(
        403,
        send(&client, "GET", "/api/v1/admin/sign-in", &member, "").status()
    );
    let shown = json_of(&send(&client, "GET", "/api/v1/admin/sign-in", &admin, ""));
    assert_eq!(
        json!({"password": true, "session_idle_days": 14, "session_max_days": 90}),
        shown["policy"]
    );
    assert_eq!(
        json!(["ana@ministry.example"]).as_array().unwrap().len(),
        shown["password_only"].as_array().unwrap().len()
    );

    let off = json!({"password": false, "session_idle_days": 1, "session_max_days": 2}).to_string();
    assert_eq!(
        403,
        send(&client, "PUT", "/api/v1/admin/sign-in", &admin, &off).status(),
        "only the owner sets it"
    );
    let bad = json!({"password": true, "session_idle_days": 30, "session_max_days": 2}).to_string();
    assert_eq!(
        400,
        send(&client, "PUT", "/api/v1/admin/sign-in", &owner, &bad).status()
    );
    let refused = send(&client, "PUT", "/api/v1/admin/sign-in", &owner, &off);
    assert_eq!(409, refused.status(), "Ana would be locked out");
    assert_eq!("would_lock_out", json_of(&refused)["error"]["code"]);

    // Short sessions: a session in use is extended by one day at most.
    let short =
        json!({"password": true, "session_idle_days": 1, "session_max_days": 2}).to_string();
    assert_eq!(
        200,
        send(&client, "PUT", "/api/v1/admin/sign-in", &owner, &short).status()
    );
    block_on(
        instance
            .store
            .execute("UPDATE core_sessions SET last_seen_at = 0", &[]),
    )
    .unwrap();
    assert_eq!(
        200,
        send(&client, "GET", "/api/v1/me", &member, "").status()
    );
    let longest = block_on(
        instance
            .store
            .query("SELECT MIN(expires_at) FROM core_sessions", &[]),
    )
    .unwrap()[0]
        .int(0)
        .unwrap();
    assert!(
        longest <= ortus_core::program::now_ms() + 24 * 60 * 60 * 1000,
        "the session lasts a day"
    );

    // Once Ana has a passkey, passwords can go.
    block_on(instance.store.execute(
        "UPDATE core_credentials SET kind = 'passkey' WHERE id = 'ana-totp'",
        &[],
    ))
    .unwrap();
    assert_eq!(
        200,
        send(&client, "PUT", "/api/v1/admin/sign-in", &owner, &off).status()
    );
    let refused = send(
        &client,
        "POST",
        "/api/v1/session/password",
        "",
        &json!({"email": "ana@ministry.example", "password": "x"}).to_string(),
    );
    assert_eq!(
        (403, "password_off"),
        (
            refused.status(),
            json_of(&refused)["error"]["code"].as_str().unwrap()
        )
    );
    let audit = json_of(&send(
        &client,
        "GET",
        "/api/v1/admin/audit?action=auth.policy_changed",
        &owner,
        "",
    ));
    assert!(audit.to_string().contains("auth.policy_changed"), "{audit}");
}

#[test]
fn policy_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    policy((fresh.open)());
}

#[test]
fn policy_on_postgres() {
    if let Some(fresh) = common::postgres() {
        policy((fresh.open)());
    }
}
