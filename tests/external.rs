//! Signing in through an outside OpenID Connect provider, against a test
//! identity provider (navikt's mock-oauth2-server, test only). Runs when
//! ORTUS_TEST_IDP holds its issuer; CI sets ORTUS_REQUIRE_IDP=1.

use std::sync::Arc;

use ortus_core::auth::accounts::{self, NewUser, Role, State};
use ortus_core::instance::testing;
use ortus_core::runtime::block_on;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};

fn idp() -> Option<String> {
    let Ok(issuer) = std::env::var("ORTUS_TEST_IDP") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_IDP").is_err(),
            "ORTUS_REQUIRE_IDP is set but ORTUS_TEST_IDP is not"
        );
        eprintln!("skipped: set ORTUS_TEST_IDP");
        return None;
    };
    Some(issuer)
}

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

/// Signs in at the test provider as `claims` and returns where it sends
/// the browser back to.
fn at_provider(authorize: &str, user: &str, claims: &Value) -> String {
    block_on(async {
        let client = ortus_core::http::client().unwrap();
        let body = format!(
            "username={}&claims={}",
            ortus_core::form::encode(user),
            ortus_core::form::encode(&claims.to_string())
        );
        let response = client
            .post(authorize)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            302,
            response.status().as_u16(),
            "the test provider did not send the browser back"
        );
        response.headers()["location"].to_str().unwrap().to_string()
    })
}

#[test]
fn a_person_signs_in_with_an_outside_provider() {
    let Some(issuer) = idp() else { return };
    let instance = testing::instance(Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    block_on(ortus_core::auth::setup::prepare(
        &instance,
        Some("t"),
        false,
        0,
    ))
    .unwrap();
    let owner = json_of(
        &client
            .post("/api/v1/setup")
            .header("X-Requested-With", "ortus")
            .body_text(r#"{"token":"t","email":"taras@ministry.example","name":"Taras"}"#)
            .send(),
    );
    let bearer = format!("Bearer {}", owner["session_token"].as_str().unwrap());
    let added = client.post("/api/v1/admin/sign-in/providers").header("Authorization", &bearer).header("X-Requested-With", "ortus")
        .body_text(&json!({"id": "eid", "name": "National eID", "issuer": issuer, "client_id": "ortus", "client_secret": "test-secret", "link_by_email": true, "create_accounts": false}).to_string())
        .send();
    assert_eq!(200, added.status(), "{}", added.body_text());
    assert!(json_of(&added)["redirect_uri"]
        .as_str()
        .unwrap()
        .ends_with("/auth/external/eid/callback"));
    let listed = json_of(&client.get("/api/v1/session/providers").send());
    assert_eq!("National eID", listed["providers"][0]["name"]);

    block_on(accounts::create(
        instance.store.as_ref(),
        &NewUser {
            email: "iryna@ministry.example",
            name: "Iryna",
            role: Role::Member,
            state: State::Active,
            expires_at: None,
        },
        1,
    ))
    .unwrap();
    let start = client.get("/auth/external/eid/start?return=%2Fmail").send();
    assert_eq!(302, start.status());
    let trip = start
        .header("Set-Cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(trip.starts_with("ortus-trip="), "{trip}");
    let trip = start
        .header("Set-Cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(trip.starts_with("ortus-trip="), "{trip}");
    let authorize = start.header("Location").unwrap().to_string();
    assert!(
        authorize.starts_with(&format!("{issuer}/authorize?"))
            && authorize.contains("code_challenge_method=S256"),
        "{authorize}"
    );

    let back = at_provider(
        &authorize,
        "iryna-subject",
        &json!({"email": "iryna@ministry.example", "email_verified": true, "name": "Iryna"}),
    );
    let path = back
        .split_once("://")
        .unwrap()
        .1
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap();
    assert!(
        path.starts_with("/auth/external/eid/callback?code="),
        "{path}"
    );
    let landed = client.get(&path).header("Cookie", &trip).send();
    assert_eq!(302, landed.status(), "{}", landed.body_text());
    assert_eq!(Some("/mail"), landed.header("Location"));
    let cookie = landed
        .header("Set-Cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let me = json_of(&client.get("/api/v1/me").header("Cookie", &cookie).send());
    assert_eq!("iryna@ministry.example", me["email"]);
    // The answer works once.
    assert_eq!(
        Some("/sign-in?external=expired"),
        client
            .get(&path)
            .header("Cookie", &trip)
            .send()
            .header("Location")
    );
    // Another browser (without the trip cookie) cannot finish a sign-in.
    let start = client.get("/auth/external/eid/start").send();
    let back = at_provider(
        start.header("Location").unwrap(),
        "iryna-subject",
        &json!({"email": "iryna@ministry.example", "email_verified": true}),
    );
    let path = back
        .split_once("://")
        .unwrap()
        .1
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap();
    assert_eq!(
        Some("/sign-in?external=expired"),
        client.get(&path).send().header("Location"),
        "no trip cookie"
    );

    // Someone the provider knows but Ortus does not: refused.
    let start = client.get("/auth/external/eid/start").send();
    let trip = start
        .header("Set-Cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let back = at_provider(
        start.header("Location").unwrap(),
        "stranger",
        &json!({"email": "stranger@elsewhere.example", "email_verified": true}),
    );
    let path = back
        .split_once("://")
        .unwrap()
        .1
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap();
    assert_eq!(
        Some("/sign-in?external=no_account"),
        client
            .get(&path)
            .header("Cookie", &trip)
            .send()
            .header("Location")
    );
}
