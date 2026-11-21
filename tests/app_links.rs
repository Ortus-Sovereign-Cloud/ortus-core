//! The files phones read to open invitation links in the Ortus app.

use std::sync::Arc;

use ortus_core::instance::testing;
use rust_web_server::test_client::TestClient;
use serde_json::Value;

#[test]
fn phones_learn_that_invitations_open_in_the_app() {
    let instance = testing::instance(Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));

    let apple = client.get("/.well-known/apple-app-site-association").send();
    assert_eq!(200, apple.status(), "{}", apple.body_text());
    let apple: Value = serde_json::from_str(apple.body_text()).unwrap();
    let details = &apple["applinks"]["details"][0];
    assert_eq!("XD7JHZ5DB3.cc.ortus.app", details["appIDs"][0], "{apple}");
    assert_eq!(
        "/welcome/accept", details["components"][0]["/"],
        "invitations"
    );
    assert_eq!("/i", details["components"][1]["/"], "the shared app link");
    assert_eq!(
        2,
        details["components"].as_array().unwrap().len(),
        "nothing else"
    );

    let android = client.get("/.well-known/assetlinks.json").send();
    assert_eq!(200, android.status());
    let android: Value = serde_json::from_str(android.body_text()).unwrap();
    assert_eq!(
        "cc.ortus.app", android[0]["target"]["package_name"],
        "{android}"
    );
    assert_eq!(
        "delegate_permission/common.handle_all_urls",
        android[0]["relation"][0]
    );

    // The apps find the installation from a work email address.
    let found = client.get("/.well-known/ortus").send();
    assert_eq!(200, found.status());
    let found: Value = serde_json::from_str(found.body_text()).unwrap();
    assert_eq!(
        instance.config.public_url,
        found["server"].as_str().unwrap()
    );

    // Nothing else under /.well-known/ comes with them.
    assert_eq!(404, client.get("/.well-known/other").send().status());
}
