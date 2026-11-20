//! Bringing people and groups in from a directory, against a test
//! OpenLDAP (ci/ldap-test.sh, test only). Runs when ORTUS_TEST_LDAP is
//! set; CI sets ORTUS_REQUIRE_LDAP=1.

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing, testing_auth::signed_in};
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

#[test]
fn people_and_groups_come_in_from_the_directory() {
    let Ok(url) = std::env::var("ORTUS_TEST_LDAP") else {
        assert!(
            std::env::var("ORTUS_REQUIRE_LDAP").is_err(),
            "ORTUS_REQUIRE_LDAP is set but ORTUS_TEST_LDAP is not"
        );
        eprintln!("skipped: run ci/ldap-test.sh and export what it prints");
        return;
    };
    let instance = testing::instance(Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let send = |method: &str, path: &str, body: &str| {
        let request = if method == "PUT" {
            client.put(path)
        } else if method == "POST" {
            client.post(path)
        } else {
            client.get(path)
        };
        request
            .header("Authorization", &format!("Bearer {owner}"))
            .header("X-Requested-With", "ortus")
            .body_text(body)
            .send()
    };
    let wrong = send("PUT", "/api/v1/admin/directory", &json!({"url": url, "bind_dn": "cn=admin,dc=ministry,dc=example", "password": "wrong", "base_dn": "dc=ministry,dc=example"}).to_string());
    assert_eq!(
        400,
        wrong.status(),
        "a wrong password is not kept: {}",
        wrong.body_text()
    );
    let saved = send("PUT", "/api/v1/admin/directory", &json!({"url": url, "bind_dn": "cn=admin,dc=ministry,dc=example", "password": "adminpass", "base_dn": "dc=ministry,dc=example"}).to_string());
    assert_eq!(200, saved.status(), "{}", saved.body_text());
    assert!(
        !saved.body_text().contains("adminpass"),
        "the password is never shown"
    );

    let synced: Value =
        serde_json::from_str(send("POST", "/api/v1/admin/directory/sync", "").body_text()).unwrap();
    assert_eq!(
        (2, 1),
        (
            synced["created"].as_i64().unwrap(),
            synced["groups"].as_i64().unwrap()
        ),
        "{synced}"
    );
    let logistics: Value =
        serde_json::from_str(send("GET", "/api/v1/admin/users?q=logistics", "").body_text())
            .unwrap();
    let emails: Vec<&str> = logistics["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["email"].as_str().unwrap())
        .collect();
    assert_eq!(
        vec!["marek@ministry.example", "olena@ministry.example"],
        emails
    );
    assert_eq!("invited", logistics["users"][0]["state"]);
    // A second sync changes nothing.
    let again: Value =
        serde_json::from_str(send("POST", "/api/v1/admin/directory/sync", "").body_text()).unwrap();
    assert_eq!(
        (0, 0),
        (
            again["created"].as_i64().unwrap(),
            again["updated"].as_i64().unwrap()
        )
    );
}
