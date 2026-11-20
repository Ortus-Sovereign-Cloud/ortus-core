//! Administering people through the API, on both engines: invitations,
//! CSV import, roles, suspension, quotas, resetting sign-in, and who may
//! do what.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store, Instance};
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
        "POST" => client.post(path),
        "PATCH" => client.patch(path),
        _ => client.delete(path),
    };
    request
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus")
        .body_text(body)
        .send()
}

fn invite_token(instance: &Arc<Instance>, email: &str) -> String {
    // The link goes by mail; tests take a fresh one straight from the
    // database by making it again.
    let token = ortus_core::auth::random_token();
    let user = block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        email,
    ))
    .unwrap()
    .unwrap();
    block_on(instance.store.execute(
        "DELETE FROM core_invites WHERE user_id = $1",
        &[user.id.as_str().into()],
    ))
    .unwrap();
    block_on(instance.store.execute(
        "INSERT INTO core_invites (token_hash, user_id, expires_at) VALUES ($1, $2, $3)",
        &[
            ortus_core::oidc::sha256_hex(&token).into(),
            user.id.as_str().into(),
            (ortus_core::program::now_ms() + 60_000).into(),
        ],
    ))
    .unwrap();
    token
}

fn people(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let admin = signed_in(&instance, "marek@ministry.example", Role::Admin);
    let member = signed_in(&instance, "ana@ministry.example", Role::Member);

    assert_eq!(
        403,
        send(&client, "GET", "/api/v1/admin/users", &member, "").status()
    );
    let invited = send(&client, "POST", "/api/v1/admin/users", &admin, &json!({"email": "iryna@ministry.example", "name": "Iryna Savchenko", "groups": ["Logistics"]}).to_string());
    assert_eq!(200, invited.status(), "{}", invited.body_text());
    assert_eq!(
        ("invited", "off"),
        (
            json_of(&invited)["state"].as_str().unwrap(),
            json_of(&invited)["sign_in"].as_str().unwrap()
        )
    );
    // An admin cannot make an owner.
    assert_eq!(
        403,
        send(
            &client,
            "POST",
            "/api/v1/admin/users",
            &admin,
            &json!({"email": "x@ministry.example", "name": "X", "role": "owner"}).to_string()
        )
        .status()
    );
    assert_eq!(
        409,
        send(
            &client,
            "POST",
            "/api/v1/admin/users",
            &admin,
            &json!({"email": "IRYNA@ministry.example", "name": "Again"}).to_string()
        )
        .status()
    );

    let csv = "email,name,role,groups\njonas@ministry.example,Jonas Berg,member,Logistics;Field\nnot-an-address,Nobody\nkatarzyna@ministry.example,Katarzyna Wójcik,admin,Board\n";
    let imported = json_of(&send(
        &client,
        "POST",
        "/api/v1/admin/users/import",
        &owner,
        csv,
    ));
    assert_eq!(2, imported["added"].as_array().unwrap().len(), "{imported}");
    assert_eq!(3, imported["failed"][0]["line"], "{imported}");

    let all = json_of(&send(&client, "GET", "/api/v1/admin/users", &admin, ""));
    assert_eq!(6, all["total"]);
    assert_eq!(3, all["groups"], "Logistics, Field and Board");
    let logistics = json_of(&send(
        &client,
        "GET",
        "/api/v1/admin/users?q=logistics",
        &admin,
        "",
    ));
    assert_eq!(
        2,
        logistics["users"].as_array().unwrap().len(),
        "{logistics}"
    );
    let unsafe_ones = json_of(&send(
        &client,
        "GET",
        "/api/v1/admin/users?filter=no_2fa",
        &admin,
        "",
    ));
    assert_eq!(
        all["without_second_factor"],
        json!(unsafe_ones["users"].as_array().unwrap().len())
    );

    // The invited person follows the link and is signed in.
    let token = invite_token(&instance, "iryna@ministry.example");
    let accepted = client
        .post("/api/v1/invitation/accept")
        .header("X-Requested-With", "ortus")
        .body_text(&json!({"token": token}).to_string())
        .send();
    assert_eq!(200, accepted.status(), "{}", accepted.body_text());
    assert_eq!("add_sign_in_method", json_of(&accepted)["next"]);
    assert_eq!(
        410,
        client
            .post("/api/v1/invitation/accept")
            .header("X-Requested-With", "ortus")
            .body_text(&json!({"token": token}).to_string())
            .send()
            .status()
    );
    let iryna_session = json_of(&accepted)["session_token"]
        .as_str()
        .unwrap()
        .to_string();
    let iryna = json_of(&accepted)["id"].as_str().unwrap().to_string();

    // Quota, suspension (which signs out), and back.
    let changed = json_of(&send(
        &client,
        "PATCH",
        &format!("/api/v1/admin/users/{iryna}"),
        &admin,
        r#"{"quota_bytes": 10737418240}"#,
    ));
    assert_eq!(10_737_418_240i64, changed["quota_bytes"].as_i64().unwrap());
    assert_eq!(
        200,
        send(
            &client,
            "PATCH",
            &format!("/api/v1/admin/users/{iryna}"),
            &admin,
            r#"{"state": "suspended"}"#
        )
        .status()
    );
    assert_eq!(
        401,
        send(&client, "GET", "/api/v1/me", &iryna_session, "").status()
    );
    assert_eq!(
        200,
        send(
            &client,
            "PATCH",
            &format!("/api/v1/admin/users/{iryna}"),
            &admin,
            r#"{"state": "active"}"#
        )
        .status()
    );

    // The last owner stays an owner; an admin cannot touch an owner.
    let owner_id = json_of(&send(&client, "GET", "/api/v1/me", &owner, ""))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        "last_owner",
        json_of(&send(
            &client,
            "PATCH",
            &format!("/api/v1/admin/users/{owner_id}"),
            &owner,
            r#"{"role": "admin"}"#
        ))["error"]["code"]
    );
    assert_eq!(
        403,
        send(
            &client,
            "PATCH",
            &format!("/api/v1/admin/users/{owner_id}"),
            &admin,
            r#"{"state": "suspended"}"#
        )
        .status()
    );

    // Resetting sign-in: signed out, invited again.
    let token2 = invite_token(&instance, "iryna@ministry.example");
    let _ = token2;
    assert_eq!(
        200,
        send(
            &client,
            "POST",
            &format!("/api/v1/admin/users/{iryna}/reset"),
            &admin,
            ""
        )
        .status()
    );
    let after = json_of(&send(
        &client,
        "GET",
        "/api/v1/admin/users?q=iryna",
        &admin,
        "",
    ));
    assert_eq!("invited", after["users"][0]["state"]);
    let audit = block_on(ortus_core::audit::list(
        instance.store.as_ref(),
        &ortus_core::audit::Filter {
            action_prefix: Some("people.".into()),
            limit: 50,
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(audit.len() >= 6, "{audit:?}");
}

#[test]
fn people_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    people((fresh.open)());
}

#[test]
fn people_on_postgres() {
    if let Some(fresh) = common::postgres() {
        people((fresh.open)());
    }
}
