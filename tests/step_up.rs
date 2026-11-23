//! Step-up, on both engines: a phone sign-in link, new recovery codes and
//! removing the authenticator need a session that proved it is its person
//! in the last ten minutes (a strong sign-in, or a confirmation); a new
//! passkey needs a session begun in the last ten minutes or confirmed.
//! A wrong confirmation is refused and counted.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;

fn step_up(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let s = instance.store.as_ref();
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let post = |path: &str| {
        client
            .post(&format!("/api/v1{path}"))
            .header("Authorization", &format!("Bearer {olena}"))
            .header("X-Requested-With", "ortus")
            .body_text("{}")
            .send()
    };
    let code = |r: &rust_web_server::test_client::TestResponse| {
        serde_json::from_str::<serde_json::Value>(r.body_text()).unwrap_or_default()["error"]
            ["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    // A fresh passkey session may.
    assert_eq!(200, post("/me/phone-link").status());

    // The same session an hour later may not, until it confirms.
    let hour_ago = ortus_core::program::now_ms() - 60 * 60 * 1000;
    block_on(s.execute(
        "UPDATE core_sessions SET created_at = $1",
        &[hour_ago.into()],
    ))
    .unwrap();
    for path in ["/me/phone-link", "/me/recovery-kit", "/me/passkeys/options"] {
        let r = post(path);
        assert_eq!(403, r.status(), "{path}: {}", r.body_text());
        assert_eq!("step_up", code(&r), "{path}");
    }
    // A wrong confirmation is refused (no password, no authenticator here).
    let wrong = client
        .post("/api/v1/session/confirm")
        .header("Authorization", &format!("Bearer {olena}"))
        .header("X-Requested-With", "ortus")
        .body_text(r#"{"password":"guess","code":"000000"}"#)
        .send();
    assert_eq!(403, wrong.status());
    assert_eq!("not_confirmed", code(&wrong));
    assert_eq!(403, post("/me/phone-link").status());
    // Confirmed (as /session/confirm records it): allowed for ten minutes.
    block_on(s.execute(
        "UPDATE core_sessions SET confirmed_at = $1",
        &[ortus_core::program::now_ms().into()],
    ))
    .unwrap();
    assert_eq!(200, post("/me/phone-link").status());
    assert_eq!(200, post("/me/passkeys/options").status());
    block_on(s.execute(
        "UPDATE core_sessions SET confirmed_at = $1",
        &[hour_ago.into()],
    ))
    .unwrap();
    assert_eq!(403, post("/me/phone-link").status(), "an old confirmation");

    // A weak session (a phone link, a recovery code) is never enough,
    // however new; it may still add its first passkey.
    let now = ortus_core::program::now_ms();
    block_on(s.execute(
        "UPDATE core_sessions SET created_at = $1, methods = 'recovery', confirmed_at = NULL",
        &[now.into()],
    ))
    .unwrap();
    assert_eq!(403, post("/me/phone-link").status());
    assert_eq!(403, post("/me/recovery-kit").status());
    assert_eq!(200, post("/me/passkeys/options").status());
    let r = post("/session/confirm/options");
    assert_eq!(200, r.status(), "{}", r.body_text());

    assert!(ortus_core::auth::step_up::strong("password+totp"));
    assert!(ortus_core::auth::step_up::strong("external:gov-id"));
    for weak in ["phone_link", "recovery", "invitation", "email", "password"] {
        assert!(!ortus_core::auth::step_up::strong(weak), "{weak}");
    }
}

#[test]
fn step_up_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    step_up((fresh.open)());
}

#[test]
fn step_up_on_postgres() {
    if let Some(fresh) = common::postgres() {
        step_up((fresh.open)());
    }
}
