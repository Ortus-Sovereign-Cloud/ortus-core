//! The SAML response check, on a real response from a test identity
//! provider (SimpleSAMLphp; tests/fixtures), and on that response bent in
//! the ways signature wrapping and its relatives bend it.

use ortus_core::auth::saml_verify::{rsa_key_of_certificate, verify};

fn fixture() -> (String, Vec<u8>) {
    let xml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/saml-response.xml"
    ))
    .unwrap();
    let cert = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/saml-idp.crt"
    ))
    .unwrap();
    (xml, rsa_key_of_certificate(&cert).unwrap())
}

#[test]
fn a_real_signed_response_verifies() {
    let (xml, key) = fixture();
    let verified = verify(&xml, &key).unwrap();
    assert!(
        verified
            .issuer
            .ends_with("/simplesaml/saml2/idp/metadata.php"),
        "{}",
        verified.issuer
    );
    assert_eq!(Some("_fixture1".to_string()), verified.in_response_to);
    assert_eq!(
        Some("http://127.0.0.1:7878/auth/saml/test/acs".to_string()),
        verified.recipient
    );
    assert!(verified
        .audiences
        .contains(&"http://127.0.0.1:7878/auth/saml/test/metadata".to_string()));
    assert!(
        verified
            .attributes
            .iter()
            .any(|(name, values)| name == "email" && values == &["user1@example.com".to_string()]),
        "{:?}",
        verified.attributes
    );
    assert_eq!(
        "urn:oasis:names:tc:SAML:2.0:status:Success",
        verified.response_status
    );
}

fn refused(xml: &str, key: &[u8], why: &str) {
    match verify(xml, key) {
        Ok(v) => panic!("{why}: accepted {v:?}"),
        Err(e) => eprintln!("{why}: refused ({e})"),
    }
}

#[test]
fn bent_responses_are_refused() {
    let (xml, key) = fixture();
    // A changed attribute.
    refused(
        &xml.replace("user1@example.com", "admin@example.com"),
        &key,
        "changed attribute",
    );
    // A comment injected into the name.
    let name_start = xml.find("<saml:NameID").unwrap();
    let close = name_start + xml[name_start..].find('>').unwrap() + 1;
    let mut commented = xml.clone();
    commented.insert_str(close + 2, "<!---->");
    refused(&commented, &key, "comment in signed content");
    // A second, unsigned assertion beside the signed one.
    let a_start = xml.find("<saml:Assertion").unwrap();
    let a_end = xml.find("</saml:Assertion>").unwrap() + "</saml:Assertion>".len();
    let assertion = &xml[a_start..a_end];
    let forged = assertion
        .replace("user1@example.com", "admin@example.com")
        .replace(" ID=\"", " ID=\"x");
    let mut doubled = xml.clone();
    doubled.insert_str(a_end, &forged);
    refused(&doubled, &key, "two assertions");
    // The signed assertion moved inside an extension, a forged one in its place.
    let mut moved = xml.clone();
    moved.replace_range(
        a_start..a_end,
        &format!("<samlp:Extensions>{assertion}</samlp:Extensions>{}", forged),
    );
    refused(&moved, &key, "assertion moved");
    // A duplicate ID.
    let id_start = xml.find("<saml:Assertion").unwrap();
    let id = &xml[id_start..]
        .split("ID=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    refused(
        &xml.replacen("<samlp:Status>", &format!("<samlp:Status ID=\"{id}\">"), 1),
        &key,
        "duplicate ID",
    );
    // The signature taken off.
    let s_start = assertion.find("<ds:Signature").unwrap();
    let s_end = assertion.find("</ds:Signature>").unwrap() + "</ds:Signature>".len();
    let mut unsigned_assertion = assertion.to_string();
    unsigned_assertion.replace_range(s_start..s_end, "");
    let response_sig_start = xml.find("<ds:Signature").unwrap();
    let mut stripped = xml.clone();
    stripped.replace_range(a_start..a_end, &unsigned_assertion);
    if response_sig_start < a_start {
        let end = stripped.find("</ds:Signature>").unwrap() + "</ds:Signature>".len();
        stripped.replace_range(response_sig_start..end, "");
    }
    refused(&stripped, &key, "no signature");
    // A KeyInfo swapped for another key changes nothing: only the
    // configured key counts, so a different key is refused.
    let other_key = rsa_key_of_certificate(include_str!("fixtures/saml-other.crt")).unwrap();
    refused(&xml, &other_key, "other key");
    // A DOCTYPE.
    refused(
        &format!("<!DOCTYPE r [<!ENTITY e \"x\">]>{xml}"),
        &key,
        "doctype",
    );
}

mod live {
    //! Against a running test identity provider (SimpleSAMLphp, test only),
    //! set up for the SP http://127.0.0.1:7878/auth/saml/test. Runs when
    //! ORTUS_TEST_SAML holds its address; CI sets ORTUS_REQUIRE_SAML=1.

    use std::sync::Arc;

    use ortus_core::auth::accounts::{self, NewUser, Role, State};
    use ortus_core::instance::testing;
    use ortus_core::runtime::block_on;
    use rust_web_server::test_client::TestClient;
    use serde_json::{json, Value};

    fn idp() -> Option<String> {
        let Ok(url) = std::env::var("ORTUS_TEST_SAML") else {
            assert!(
                std::env::var("ORTUS_REQUIRE_SAML").is_err(),
                "ORTUS_REQUIRE_SAML is set but ORTUS_TEST_SAML is not"
            );
            eprintln!("skipped: set ORTUS_TEST_SAML");
            return None;
        };
        Some(url)
    }

    /// Follows the identity provider's pages as a browser would, signs in
    /// as user1, and returns the SAMLResponse it posts back.
    fn sign_in_at_idp(start: &str) -> String {
        block_on(async {
            let client = ortus_core::http::client().unwrap();
            let mut cookies: Vec<String> = Vec::new();
            let mut url = start.to_string();
            let mut page = String::new();
            for _ in 0..6 {
                let response = client
                    .get(&url)
                    .header("Cookie", cookies.join("; "))
                    .send()
                    .await
                    .unwrap();
                for c in response.headers().get_all("set-cookie") {
                    cookies.push(c.to_str().unwrap().split(';').next().unwrap().to_string());
                }
                if let Some(next) = response.headers().get("location") {
                    let next = next.to_str().unwrap().to_string();
                    url = if next.starts_with("http") {
                        next
                    } else {
                        format!("{}{}", url.split("/simplesaml").next().unwrap(), next)
                    };
                    continue;
                }
                page = response.text().await.unwrap();
                break;
            }
            let state = ortus_core::form::query(&url, "AuthState").expect("a login page");
            let body = format!(
                "username=user1&password=password&AuthState={}",
                ortus_core::form::encode(&state)
            );
            let answer = client
                .post(url.split('?').next().unwrap())
                .header("Cookie", cookies.join("; "))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body)
                .send()
                .await
                .unwrap();
            page = answer.text().await.unwrap_or(page);
            let value = page
                .split("name=\"SAMLResponse\" value=\"")
                .nth(1)
                .expect("an answer form")
                .split('"')
                .next()
                .unwrap();
            value
                .replace("&#x2B;", "+")
                .replace("&#x3D;", "=")
                .replace("&#x2F;", "/")
                .replace("&#43;", "+")
                .replace("&#61;", "=")
                .replace("&#47;", "/")
        })
    }

    #[test]
    fn a_person_signs_in_with_a_saml_provider() {
        let Some(idp) = idp() else { return };
        let instance = testing::instance(Vec::new());
        assert_eq!(
            "http://127.0.0.1:7878", instance.config.public_url,
            "the test provider knows this address"
        );
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
        block_on(ortus_core::auth::setup::prepare(
            &instance,
            Some("t"),
            false,
            0,
        ))
        .unwrap();
        let owner: Value = serde_json::from_str(
            client
                .post("/api/v1/setup")
                .header("X-Requested-With", "ortus")
                .body_text(r#"{"token":"t","email":"taras@ministry.example","name":"Taras"}"#)
                .send()
                .body_text(),
        )
        .unwrap();
        let bearer = format!("Bearer {}", owner["session_token"].as_str().unwrap());
        let metadata = block_on(async {
            ortus_core::http::client()
                .unwrap()
                .get(format!("{idp}/simplesaml/saml2/idp/metadata.php"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        let certificate = metadata
            .split("<ds:X509Certificate>")
            .nth(1)
            .unwrap()
            .split('<')
            .next()
            .unwrap();
        let added = client.post("/api/v1/admin/sign-in/saml").header("Authorization", &bearer).header("X-Requested-With", "ortus")
            .body_text(&json!({"id": "test", "name": "Old directory", "entity_id": format!("{idp}/simplesaml/saml2/idp/metadata.php"), "sso_url": format!("{idp}/simplesaml/saml2/idp/SSOService.php"), "certificate": certificate, "link_by_email": true, "create_accounts": false}).to_string())
            .send();
        assert_eq!(200, added.status(), "{}", added.body_text());
        block_on(accounts::create(
            instance.store.as_ref(),
            &NewUser {
                email: "user1@example.com",
                name: "User One",
                role: Role::Member,
                state: State::Active,
                expires_at: None,
            },
            1,
        ))
        .unwrap();

        let start = client.get("/auth/saml/test/start?return=%2Fmail").send();
        assert_eq!(302, start.status());
        let binding = start
            .header("Set-Cookie")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let answer = sign_in_at_idp(start.header("Location").unwrap());
        let post = |cookie: Option<&str>| {
            let mut request = client
                .post("/auth/saml/test/acs")
                .header("Content-Type", "application/x-www-form-urlencoded");
            if let Some(cookie) = cookie {
                request = request.header("Cookie", cookie);
            }
            request
                .body_text(&format!(
                    "SAMLResponse={}",
                    ortus_core::form::encode(&answer)
                ))
                .send()
        };
        // With the browser binding set at the start: signed in.
        let landed = post(Some(&binding));
        assert_eq!(
            Some("/mail"),
            landed.header("Location"),
            "{}",
            landed.body_text()
        );
        let cookie = landed
            .header("Set-Cookie")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let me: Value = serde_json::from_str(
            client
                .get("/api/v1/me")
                .header("Cookie", &cookie)
                .send()
                .body_text(),
        )
        .unwrap();
        assert_eq!("user1@example.com", me["email"]);
        // The same answer again: refused.
        assert_eq!(
            Some("/sign-in?external=expired"),
            post(Some(&binding)).header("Location")
        );

        // Another browser cannot finish a sign-in it did not start.
        let start = client.get("/auth/saml/test/start").send();
        let answer = sign_in_at_idp(start.header("Location").unwrap());
        let foreign = client
            .post("/auth/saml/test/acs")
            .body_text(&format!(
                "SAMLResponse={}",
                ortus_core::form::encode(&answer)
            ))
            .send();
        assert_eq!(
            Some("/sign-in?external=expired"),
            foreign.header("Location")
        );
    }
}
