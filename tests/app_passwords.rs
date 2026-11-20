//! App passwords, on both engines: made and shown once, checked as apps
//! type them, limited like sign-in, gone when removed or suspended.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::auth::app_passwords::{authenticate, Refused};
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

fn app_passwords(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let send = |method: &str, path: &str, body: Value| {
        let request = match method {
            "GET" => client.get(path),
            "DELETE" => client.delete(path),
            _ => client.post(path),
        };
        request
            .header("Authorization", &format!("Bearer {olena}"))
            .header("X-Requested-With", "ortus")
            .body_text(&body.to_string())
            .send()
    };
    let made: Value = serde_json::from_str(
        send(
            "POST",
            "/api/v1/me/app-passwords",
            json!({"name": "Thunderbird on the laptop"}),
        )
        .body_text(),
    )
    .unwrap();
    let password = made["password"].as_str().unwrap().to_string();
    let listed = send("GET", "/api/v1/me/app-passwords", Value::Null)
        .body_text()
        .to_string();
    assert!(
        listed.contains("Thunderbird on the laptop") && !listed.contains(&password),
        "shown once only: {listed}"
    );

    // As apps type it: with dashes, spaces, capitals.
    let user = block_on(authenticate(
        &instance,
        "olena@ministry.example",
        &password,
        "10.0.0.5",
    ))
    .unwrap();
    assert_eq!("olena@ministry.example", user.email);
    let spaced = password.replace('-', " ").to_uppercase();
    assert!(block_on(authenticate(
        &instance,
        "OLENA@ministry.example",
        &spaced,
        "10.0.0.5"
    ))
    .is_ok());
    assert_eq!(
        Err(Refused::Wrong),
        block_on(authenticate(
            &instance,
            "olena@ministry.example",
            "abcd-efgh-jkmn-pqrs",
            "10.0.0.5"
        ))
        .map(|u| u.id)
    );
    assert_eq!(
        Err(Refused::Wrong),
        block_on(authenticate(
            &instance,
            "nobody@ministry.example",
            &password,
            "10.0.0.5"
        ))
        .map(|u| u.id)
    );

    // Many wrong tries from one network: blocked, even with the right one.
    for _ in 0..6 {
        let _ = block_on(authenticate(
            &instance,
            "olena@ministry.example",
            "wrong",
            "10.0.0.9",
        ));
    }
    assert!(matches!(
        block_on(authenticate(
            &instance,
            "olena@ministry.example",
            &password,
            "10.0.0.9"
        )),
        Err(Refused::Blocked(_))
    ));

    // (The tries also count against her address; a new day, say.)
    block_on(
        instance
            .store
            .execute("DELETE FROM core_signin_limits", &[]),
    )
    .unwrap();

    // Removed: the app can no longer sign in. Suspended: neither.
    let second: Value = serde_json::from_str(
        send("POST", "/api/v1/me/app-passwords", json!({"name": "Phone"})).body_text(),
    )
    .unwrap();
    let id = made["id"].as_str().unwrap();
    assert_eq!(
        200,
        send(
            "DELETE",
            &format!("/api/v1/me/app-passwords/{id}"),
            Value::Null
        )
        .status()
    );
    assert_eq!(
        Err(Refused::Wrong),
        block_on(authenticate(
            &instance,
            "olena@ministry.example",
            &password,
            "10.0.0.6"
        ))
        .map(|u| u.id)
    );
    let phone = second["password"].as_str().unwrap();
    assert!(block_on(authenticate(
        &instance,
        "olena@ministry.example",
        phone,
        "10.0.0.6"
    ))
    .is_ok());
    block_on(instance.store.execute(
        "UPDATE core_users SET state = 'suspended' WHERE email = 'olena@ministry.example'",
        &[],
    ))
    .unwrap();
    assert_eq!(
        Err(Refused::Wrong),
        block_on(authenticate(
            &instance,
            "olena@ministry.example",
            phone,
            "10.0.0.6"
        ))
        .map(|u| u.id)
    );
}

#[test]
fn app_passwords_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    app_passwords((fresh.open)());
}

#[test]
fn app_passwords_on_postgres() {
    if let Some(fresh) = common::postgres() {
        app_passwords((fresh.open)());
    }
}
