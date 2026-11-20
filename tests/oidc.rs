//! Ortus as an OpenID Connect provider: a registered application signs a
//! person in with the code flow and PKCE, and checks the ID token against
//! the published keys.

mod common;

use std::sync::Arc;

use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ortus_core::instance::testing;
use ortus_core::oidc::challenge_of;
use ortus_core::runtime::block_on;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

fn param(location: &str, name: &str) -> Option<String> {
    ortus_core::form::query(location, name)
}

#[test]
fn an_application_signs_a_person_in_with_ortus() {
    let instance = testing::instance(Vec::new());
    let issuer = instance.config.public_url.clone();
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
            .body_text(r#"{"token":"t","email":"taras@ministry.example","name":"Taras Bondar"}"#)
            .send(),
    );
    let session = owner["session_token"].as_str().unwrap().to_string();
    let bearer = format!("Bearer {session}");

    let discovery = json_of(&client.get("/.well-known/openid-configuration").send());
    assert_eq!(issuer, discovery["issuer"]);
    assert_eq!(
        json!(["S256"]),
        discovery["code_challenge_methods_supported"]
    );

    let added = json_of(&client.post("/api/v1/admin/oidc/clients").header("Authorization", &bearer).header("X-Requested-With", "ortus")
        .body_text(r#"{"name":"Case files","redirect_uris":["https://cases.example/cb"],"confidential":true}"#).send());
    let (id, secret) = (
        added["client_id"].as_str().unwrap().to_string(),
        added["client_secret"].as_str().unwrap().to_string(),
    );

    let verifier = "a-long-random-verifier-of-forty-three-characters-or-more";
    let authorize = format!(
        "/oauth2/authorize?response_type=code&client_id={id}&redirect_uri=https%3A%2F%2Fcases.example%2Fcb&scope=openid%20email%20profile&state=s1&nonce=n1&code_challenge={}&code_challenge_method=S256",
        challenge_of(verifier)
    );
    // Not signed in: to the sign-in page, and back to this request after.
    let anonymous = client.get(&authorize).send();
    assert_eq!(302, anonymous.status());
    let to_sign_in = anonymous.header("Location").unwrap();
    assert!(
        to_sign_in.starts_with("/sign-in?return=%2Foauth2%2Fauthorize"),
        "{to_sign_in}"
    );
    // Signed in: straight back to the application with a code.
    let back = client
        .get(&authorize)
        .header("Cookie", &format!("ortus={session}"))
        .send();
    assert_eq!(302, back.status());
    let location = back.header("Location").unwrap().to_string();
    assert!(
        location.starts_with("https://cases.example/cb?code="),
        "{location}"
    );
    assert_eq!(Some("s1".into()), param(&location, "state"));
    let code = param(&location, "code").unwrap();

    let exchange = |code: &str, verifier: &str, secret: &str| {
        client.post("/oauth2/token")
            .header("Authorization", &format!("Basic {}", STANDARD.encode(format!("{id}:{secret}"))))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body_text(&format!("grant_type=authorization_code&code={code}&redirect_uri=https%3A%2F%2Fcases.example%2Fcb&code_verifier={verifier}"))
            .send()
    };
    assert_eq!(
        "invalid_client",
        json_of(&exchange(&code, verifier, "wrong"))["error"]
    );
    let tokens = exchange(&code, verifier, &secret);
    assert_eq!(200, tokens.status(), "{}", tokens.body_text());
    let tokens = json_of(&tokens);
    // A code works once.
    assert_eq!(
        "invalid_grant",
        json_of(&exchange(&code, verifier, &secret))["error"]
    );

    // The ID token is signed by a published key.
    let id_token = tokens["id_token"].as_str().unwrap();
    let parts: Vec<&str> = id_token.split('.').collect();
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    let keys = json_of(&client.get("/oauth2/jwks").send());
    let key = keys["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kid"] == header["kid"])
        .unwrap();
    let mut point = vec![4u8];
    point.extend(URL_SAFE_NO_PAD.decode(key["x"].as_str().unwrap()).unwrap());
    point.extend(URL_SAFE_NO_PAD.decode(key["y"].as_str().unwrap()).unwrap());
    let signed = format!("{}.{}", parts[0], parts[1]);
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
        .verify(
            signed.as_bytes(),
            &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
        )
        .expect("a good signature");
    assert_eq!(
        (issuer.as_str(), id.as_str(), "n1"),
        (
            claims["iss"].as_str().unwrap(),
            claims["aud"].as_str().unwrap(),
            claims["nonce"].as_str().unwrap()
        )
    );
    assert_eq!(
        ("taras@ministry.example", "Taras Bondar"),
        (
            claims["email"].as_str().unwrap(),
            claims["name"].as_str().unwrap()
        )
    );

    let info = json_of(
        &client
            .get("/oauth2/userinfo")
            .header(
                "Authorization",
                &format!("Bearer {}", tokens["access_token"].as_str().unwrap()),
            )
            .send(),
    );
    assert_eq!(claims["sub"], info["sub"]);
    assert_eq!(
        401,
        client
            .get("/oauth2/userinfo")
            .header("Authorization", "Bearer nonsense")
            .send()
            .status()
    );

    // A wrong verifier, an unregistered return address.
    let again = client
        .get(&authorize)
        .header("Cookie", &format!("ortus={session}"))
        .send();
    let code = param(again.header("Location").unwrap(), "code").unwrap();
    assert_eq!(
        "invalid_grant",
        json_of(&exchange(
            &code,
            "some-other-verifier-that-is-also-quite-long-enough",
            &secret
        ))["error"]
    );
    let elsewhere = authorize.replace("cases.example", "evil.example");
    let refused = client
        .get(&elsewhere)
        .header("Cookie", &format!("ortus={session}"))
        .send();
    assert_eq!(400, refused.status());
    assert!(
        refused.header("Location").is_none(),
        "never sent to an unregistered address"
    );
    // Without PKCE the application gets an error back.
    let no_pkce = authorize
        .split("&code_challenge=")
        .next()
        .unwrap()
        .to_string();
    let answer = client
        .get(&no_pkce)
        .header("Cookie", &format!("ortus={session}"))
        .send();
    assert!(answer
        .header("Location")
        .unwrap()
        .contains("error=invalid_request"));
}

fn one_signing_key(store: Arc<dyn ortus_core::storage::AsyncStore>) {
    block_on(ortus_core::storage::install(
        store.as_ref(),
        &ortus_core::schemas(),
        0,
    ))
    .unwrap();
    let first = block_on(ortus_core::oidc::SigningKey::current(store.as_ref(), 1)).unwrap();
    let again = block_on(ortus_core::oidc::SigningKey::current(store.as_ref(), 2)).unwrap();
    assert_eq!(first.kid, again.kid);
    assert_eq!(first.jwk(), again.jwk());
}

#[test]
fn the_signing_key_is_made_once_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    one_signing_key((fresh.open)());
}

#[test]
fn the_signing_key_is_made_once_on_postgres() {
    if let Some(fresh) = common::postgres() {
        one_signing_key((fresh.open)());
    }
}
