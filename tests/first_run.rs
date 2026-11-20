//! The admin overview's steps follow what is really there, on both engines.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::Value;

fn first_run(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let member = signed_in(&instance, "ana@ministry.example", Role::Member);
    let send = |method: &str, path: &str, token: &str| {
        let request = if method == "POST" {
            client.post(path)
        } else {
            client.get(path)
        };
        request
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .send()
    };
    let states = |v: &Value| {
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["state"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };

    assert_eq!(403, send("GET", "/api/v1/admin/overview", &member).status());
    let first: Value =
        serde_json::from_str(send("GET", "/api/v1/admin/overview", &owner).body_text()).unwrap();
    // Two people already (the owner and Ana); nothing else yet.
    assert_eq!(
        vec!["next", "later", "done", "later", "later", "later"],
        states(&first),
        "{first}"
    );
    assert_eq!(1, first["owners"]);
    assert!(
        !first.to_string().contains("secret"),
        "no secrets in the facts"
    );

    let me = block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        "taras@ministry.example",
    ))
    .unwrap()
    .unwrap();
    block_on(instance.store.execute(
        "INSERT INTO core_credentials (id, user_id, kind, name, data, created_at) VALUES ('kit', $1, 'recovery', 'Recovery', 'x', 1)",
        &[me.id.as_str().into()],
    ))
    .unwrap();
    assert_eq!(
        200,
        send("POST", "/api/v1/admin/overview/data-confirmed", &owner).status()
    );
    block_on(ortus_core::auth::policy::set_setting(
        instance.store.as_ref(),
        "backup_checked_at",
        "1",
    ))
    .unwrap();
    let later: Value =
        serde_json::from_str(send("GET", "/api/v1/admin/overview", &owner).body_text()).unwrap();
    // The test address is http://localhost, so the domain step stays next.
    assert_eq!(
        vec!["done", "next", "done", "later", "done", "done"],
        states(&later),
        "{later}"
    );
    assert_eq!(4, later["done"]);
}

#[test]
fn first_run_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    first_run((fresh.open)());
}

#[test]
fn first_run_on_postgres() {
    if let Some(fresh) = common::postgres() {
        first_run((fresh.open)());
    }
}
