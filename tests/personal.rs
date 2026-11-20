//! One's own settings and first day, on both engines.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::auth::sessions::{self, Lifetime};
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

fn personal(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let iryna = signed_in(&instance, "iryna@ministry.example", Role::Member);
    let send = |method: &str, path: &str, token: &str, body: &str| {
        let request = match method {
            "GET" => client.get(path),
            "PUT" => client.put(path),
            "DELETE" => client.delete(path),
            "PATCH" => client.patch(path),
            _ => client.post(path),
        };
        request
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .body_text(body)
            .send()
    };
    let read = |method: &str, path: &str, token: &str, body: &str| -> Value {
        let response = send(method, path, token, body);
        assert_eq!(
            200,
            response.status(),
            "{method} {path}: {}",
            response.body_text()
        );
        serde_json::from_str(response.body_text()).unwrap()
    };

    // Preferences.
    assert_eq!(
        json!({"language": null, "text_size": 100, "motion": "system"}),
        read("GET", "/api/v1/me/preferences", &iryna, "")
    );
    let changed = read(
        "PUT",
        "/api/v1/me/preferences",
        &iryna,
        &json!({"language": "uk", "text_size": 125, "motion": "reduce"}).to_string(),
    );
    assert_eq!(
        json!({"language": "uk", "text_size": 125, "motion": "reduce"}),
        changed
    );
    assert_eq!("uk", read("GET", "/api/v1/me", &iryna, "")["language"]);
    let back = read(
        "PUT",
        "/api/v1/me/preferences",
        &iryna,
        &json!({"language": null}).to_string(),
    );
    assert_eq!(
        (Value::Null, 125),
        (
            back["language"].clone(),
            back["text_size"].as_i64().unwrap()
        ),
        "null follows the browser; the rest stays"
    );
    for bad in [
        json!({"language": "xx"}),
        json!({"text_size": 300}),
        json!({"motion": "fast"}),
    ] {
        assert_eq!(
            400,
            send("PUT", "/api/v1/me/preferences", &iryna, &bad.to_string()).status(),
            "{bad}"
        );
    }

    // Name.
    read(
        "PUT",
        "/api/v1/me/profile",
        &iryna,
        &json!({"name": "  Iryna Savchenko "}).to_string(),
    );
    assert_eq!(
        "Iryna Savchenko",
        read("GET", "/api/v1/me", &iryna, "")["name"]
    );
    assert_eq!(
        400,
        send(
            "PUT",
            "/api/v1/me/profile",
            &iryna,
            &json!({"name": " "}).to_string()
        )
        .status()
    );

    // Devices: a second one, then signed out from the first.
    let user = block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        "iryna@ministry.example",
    ))
    .unwrap()
    .unwrap();
    let phone = block_on(sessions::start(
        instance.store.as_ref(),
        &user.id,
        "passkey",
        "10.0.0.7",
        "Phone",
        Lifetime::default(),
        1,
    ))
    .unwrap();
    let listed = read("GET", "/api/v1/me/sessions", &iryna, "");
    let list = listed["sessions"].as_array().unwrap();
    assert_eq!(2, list.len());
    let other = list.iter().find(|s| s["current"] == false).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        404,
        send(
            "DELETE",
            &format!("/api/v1/me/sessions/{other}"),
            &owner,
            ""
        )
        .status(),
        "only one's own devices"
    );
    read(
        "DELETE",
        &format!("/api/v1/me/sessions/{other}"),
        &iryna,
        "",
    );
    assert_eq!(
        401,
        send("GET", "/api/v1/me", &phone, "").status(),
        "the phone is signed out"
    );

    // First day: one group with a colleague, and the owner for help.
    let invited = send(
        "POST",
        "/api/v1/admin/users",
        &owner,
        &json!({"email": "olena@ministry.example", "name": "Olena Hrytsenko", "groups": ["Logistics"]}).to_string(),
    );
    assert_eq!(200, invited.status(), "{}", invited.body_text());
    block_on(instance.store.execute(
        "UPDATE core_users SET state = 'active' WHERE email = 'olena@ministry.example'",
        &[],
    ))
    .unwrap();
    block_on(ortus_core::auth::admin_users::join_group(
        instance.store.as_ref(),
        "Logistics",
        &user.id,
    ))
    .unwrap();
    let first = read("GET", "/api/v1/me/welcome", &iryna, "");
    assert_eq!(
        json!([{"id": "secure", "done": false}, {"id": "policies", "done": false}, {"id": "team", "done": false}]),
        first["steps"]
    );
    assert_eq!(json!(["Logistics"]), first["groups"]);
    assert_eq!(
        json!([{"name": "Olena Hrytsenko", "email": "olena@ministry.example", "group": "Logistics"}, {"name": "taras@ministry.example", "email": "taras@ministry.example", "help": true}]),
        first["people"]
    );
    let after = read("POST", "/api/v1/me/welcome/policies", &iryna, "");
    assert_eq!(true, after["steps"][1]["done"]);
    assert_eq!(
        404,
        send("POST", "/api/v1/me/welcome/other", &iryna, "").status()
    );
}

#[test]
fn personal_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    personal((fresh.open)());
}

#[test]
fn personal_on_postgres() {
    if let Some(fresh) = common::postgres() {
        personal((fresh.open)());
    }
}
