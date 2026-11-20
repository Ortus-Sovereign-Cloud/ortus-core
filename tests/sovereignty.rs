//! The Sovereignty facts and the compliance report, on both engines.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::Value;

fn sovereignty(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let member = signed_in(&instance, "sam@ministry.example", Role::Member);
    let get = |path: &str, token: &str| {
        client
            .get(path)
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .send()
    };

    assert_eq!(403, get("/api/v1/admin/sovereignty", &member).status());
    let facts: Value =
        serde_json::from_str(get("/api/v1/admin/sovereignty", &owner).body_text()).unwrap();
    assert_eq!("AES-256-GCM", facts["keys"]["algorithm"]);
    assert_eq!(4, facts["labels"].as_array().unwrap().len());
    assert_eq!(
        serde_json::json!([]),
        facts["outbound"]["calls"],
        "nothing called yet"
    );
    assert!(!facts.to_string().contains("secret_key") && !facts.to_string().contains("password"));

    // A call to a host nobody configured shows as unexpected; once the
    // directory is configured there, it does not.
    block_on(ortus_core::outbound::note(
        instance.store.as_ref(),
        "directory",
        "ldaps://hr.ministry.example:636",
    ));
    block_on(ortus_core::outbound::note(
        instance.store.as_ref(),
        "directory",
        "ldaps://hr.ministry.example:636",
    ));
    let facts: Value =
        serde_json::from_str(get("/api/v1/admin/sovereignty", &owner).body_text()).unwrap();
    assert_eq!(2, facts["outbound"]["calls"][0]["calls"]);
    assert_eq!(
        "hr.ministry.example:636",
        facts["outbound"]["unexpected"][0]["host"]
    );
    block_on(instance.store.execute(
        "INSERT INTO core_directory (id, url, bind_dn, password, base_dn, user_filter, group_filter, mail_attribute, name_attribute) VALUES (1, 'ldaps://hr.ministry.example:636', 'cn=a', 'x', 'dc=x', 'f', 'g', 'mail', 'cn')",
        &[],
    ))
    .unwrap();
    let facts: Value =
        serde_json::from_str(get("/api/v1/admin/sovereignty", &owner).body_text()).unwrap();
    assert_eq!(serde_json::json!([]), facts["outbound"]["unexpected"]);

    // The report: a file, with the audit chain verified now.
    let report = get("/api/v1/admin/sovereignty/report", &owner);
    assert_eq!(200, report.status());
    assert!(report
        .header("content-disposition")
        .unwrap_or_default()
        .contains("ortus-compliance-report.json"));
    let report: Value = serde_json::from_str(report.body_text()).unwrap();
    assert_eq!(Value::Null, report["audit"]["verified"]["broken_at"]);
    assert_eq!("taras@ministry.example", report["generated_by"]);
    let after: Value =
        serde_json::from_str(get("/api/v1/admin/sovereignty", &owner).body_text()).unwrap();
    assert!(
        after["audit"]["last_check"]["at"].as_i64().is_some(),
        "the screen shows the last check"
    );
}

#[test]
fn sovereignty_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    sovereignty((fresh.open)());
}

#[test]
fn sovereignty_on_postgres() {
    if let Some(fresh) = common::postgres() {
        sovereignty((fresh.open)());
    }
}
