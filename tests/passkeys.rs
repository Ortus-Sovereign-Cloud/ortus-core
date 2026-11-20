//! Passkeys through the API, with a software authenticator: adding one
//! while signed in, signing in with it, and what is refused.

use std::sync::Arc;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ortus_core::instance::testing;
use ortus_core::runtime::block_on;
use rust_web_server::application::Application;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

struct Authenticator {
    pair: EcdsaKeyPair,
    id: Vec<u8>,
    count: u32,
    user_handle: Vec<u8>,
}

impl Authenticator {
    fn new() -> Self {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .unwrap();
        Self {
            pair: EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref())
                .unwrap(),
            id: vec![7; 16],
            count: 0,
            user_handle: Vec::new(),
        }
    }

    fn auth_data(&mut self, rp_id: &str, attested: bool) -> Vec<u8> {
        self.count += 1;
        let mut out = Sha256::digest(rp_id.as_bytes()).to_vec();
        out.push(0x01 | 0x04 | if attested { 0x40 } else { 0 });
        out.extend_from_slice(&self.count.to_be_bytes());
        if attested {
            out.extend_from_slice(&[0u8; 16]);
            out.extend_from_slice(&(self.id.len() as u16).to_be_bytes());
            out.extend_from_slice(&self.id);
            let point = self.pair.public_key().as_ref();
            out.extend_from_slice(&[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20]);
            out.extend_from_slice(&point[1..33]);
            out.extend_from_slice(&[0x22, 0x58, 0x20]);
            out.extend_from_slice(&point[33..65]);
        }
        out
    }

    /// The answer to `navigator.credentials.create(options)`.
    fn create(&mut self, options: &Value, origin: &str) -> Value {
        self.user_handle = URL_SAFE_NO_PAD
            .decode(options["user"]["id"].as_str().unwrap())
            .unwrap();
        let client =
            json!({"type": "webauthn.create", "challenge": options["challenge"], "origin": origin})
                .to_string();
        let data = self.auth_data(options["rp"]["id"].as_str().unwrap(), true);
        let mut attestation = vec![
            0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e', 0x67,
        ];
        attestation.extend_from_slice(b"attStmt");
        attestation.extend_from_slice(&[0xa0, 0x68]);
        attestation.extend_from_slice(b"authData");
        attestation.extend_from_slice(&[0x59, (data.len() >> 8) as u8, data.len() as u8]);
        attestation.extend_from_slice(&data);
        json!({"name": "Test key", "clientDataJSON": b64(client.as_bytes()), "attestationObject": b64(&attestation)})
    }

    /// The answer to `navigator.credentials.get(options)`.
    fn get(&mut self, options: &Value, origin: &str) -> Value {
        let client =
            json!({"type": "webauthn.get", "challenge": options["challenge"], "origin": origin})
                .to_string();
        let data = self.auth_data(options["rpId"].as_str().unwrap(), false);
        let mut message = data.clone();
        message.extend_from_slice(&Sha256::digest(client.as_bytes()));
        let signature = self.pair.sign(&SystemRandom::new(), &message).unwrap();
        json!({
            "id": b64(&self.id),
            "clientDataJSON": b64(client.as_bytes()),
            "authenticatorData": b64(&data),
            "signature": b64(signature.as_ref()),
            "userHandle": b64(&self.user_handle),
        })
    }
}

fn post<A: Application>(
    client: &TestClient<A>,
    path: &str,
    token: Option<&str>,
    body: &Value,
) -> TestResponse {
    let mut request = client.post(path).header("X-Requested-With", "ortus");
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    request.body_text(&body.to_string()).send()
}

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

#[test]
fn a_passkey_is_added_and_signs_in() {
    let instance = testing::instance(Vec::new());
    let origin = instance.config.public_url.clone();
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    block_on(ortus_core::auth::setup::prepare(
        &instance,
        Some("t"),
        false,
        0,
    ))
    .unwrap();
    let owner = json_of(&post(
        &client,
        "/api/v1/setup",
        None,
        &json!({"token": "t", "email": "taras@ministry.example", "name": "Taras"}),
    ));
    let token = owner["session_token"].as_str().unwrap();

    let mut key = Authenticator::new();
    let options = json_of(&post(
        &client,
        "/api/v1/me/passkeys/options",
        Some(token),
        &json!({}),
    ));
    assert_eq!(
        "required",
        options["authenticatorSelection"]["userVerification"]
    );
    let answer = key.create(&options, &origin);
    let added = post(&client, "/api/v1/me/passkeys", Some(token), &answer);
    assert_eq!(200, added.status(), "{}", added.body_text());
    // The same challenge works once.
    assert_eq!(
        "passkey_refused",
        json_of(&post(&client, "/api/v1/me/passkeys", Some(token), &answer))["error"]["code"]
    );

    // It is the only way to sign in, so it cannot be removed.
    let id = json_of(&added)["id"].as_str().unwrap().to_string();
    let removal = client
        .delete(&format!("/api/v1/me/passkeys/{id}"))
        .header("X-Requested-With", "ortus")
        .header("Authorization", &format!("Bearer {token}"))
        .send();
    assert_eq!(409, removal.status(), "{}", removal.body_text());

    let options = json_of(&post(
        &client,
        "/api/v1/session/passkey/options",
        None,
        &json!({}),
    ));
    let answer = key.get(&options, &origin);
    let signed = post(&client, "/api/v1/session/passkey", None, &answer);
    assert_eq!(200, signed.status(), "{}", signed.body_text());
    assert_eq!("taras@ministry.example", json_of(&signed)["email"]);
    assert!(signed.header("Set-Cookie").is_some());

    // A replayed answer is refused: its challenge is used up.
    assert_eq!(
        401,
        post(&client, "/api/v1/session/passkey", None, &answer).status()
    );
    // An answer for another site is refused.
    let options = json_of(&post(
        &client,
        "/api/v1/session/passkey/options",
        None,
        &json!({}),
    ));
    let wrong = key.get(&options, "https://evil.example");
    assert_eq!(
        "passkey_refused",
        json_of(&post(&client, "/api/v1/session/passkey", None, &wrong))["error"]["code"]
    );
    // An unknown passkey is refused.
    let options = json_of(&post(
        &client,
        "/api/v1/session/passkey/options",
        None,
        &json!({}),
    ));
    let mut other = Authenticator::new();
    other.id = vec![9; 16];
    let unknown = other.get(&options, &origin);
    assert_eq!(
        401,
        post(&client, "/api/v1/session/passkey", None, &unknown).status()
    );

    let security = client
        .get("/api/v1/me/security")
        .header("Authorization", &format!("Bearer {token}"))
        .send();
    assert_eq!("Test key", json_of(&security)["passkeys"][0]["name"]);
}
