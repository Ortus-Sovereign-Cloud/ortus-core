//! Review S8, on both engines: an administrator resets a member's sign-in,
//! but not another administrator's (the reset hands its doer the new
//! link); an owner resets an administrator.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;

fn reset(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let anna = signed_in(&instance, "anna@ministry.example", Role::Admin);
    signed_in(&instance, "boris@ministry.example", Role::Admin);
    signed_in(&instance, "olena@ministry.example", Role::Member);
    let id = |email: &str| {
        block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            email,
        ))
        .unwrap()
        .unwrap()
        .id
    };
    let reset = |token: &str, who: &str| {
        client
            .post(&format!("/api/v1/admin/users/{}/reset", id(who)))
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .body_text("")
            .send()
            .status()
    };
    assert_eq!(200, reset(&anna, "olena@ministry.example"), "a member");
    assert_eq!(
        403,
        reset(&anna, "boris@ministry.example"),
        "another administrator"
    );
    assert_eq!(
        200,
        reset(&owner, "boris@ministry.example"),
        "the owner may"
    );
}

#[test]
fn reset_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    reset((fresh.open)());
}

#[test]
fn reset_on_postgres() {
    if let Some(fresh) = common::postgres() {
        reset((fresh.open)());
    }
}
