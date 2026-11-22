//! Signing in on one's phone from the web, on both engines: a signed-in
//! person makes a one-time link; the app follows it once and is signed in
//! as them; it expires; a newer link replaces an older one; a suspended
//! account's link does not work.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;

fn with<'a, A: rust_web_server::application::Application>(
    token: &str,
    r: rust_web_server::test_client::TestRequest<'a, A>,
) -> rust_web_server::test_client::TestRequest<'a, A> {
    r.header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus")
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

/// The code from a link's `p=`.
fn code(link: &str) -> String {
    link.rsplit_once("p=").unwrap().1.to_string()
}

fn phone_link(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let follow = |token: &str| {
        client
            .post("/api/v1/session/phone-link")
            .header("X-Requested-With", "ortus")
            .header("User-Agent", "Ortus/1 CFNetwork/3826 Darwin/25.0.0")
            .body_text(&format!("{{\"token\":\"{token}\"}}"))
            .send()
    };

    // Only someone signed in makes one.
    assert_eq!(
        401,
        client
            .post("/api/v1/me/phone-link")
            .header("X-Requested-With", "ortus")
            .body_text("{}")
            .send()
            .status()
    );
    let made = with(&olena, client.post("/api/v1/me/phone-link"))
        .body_text("{}")
        .send();
    assert_eq!(200, made.status(), "{}", made.body_text());
    let made = json(made.body_text());
    let link = made["link"].as_str().unwrap();
    assert!(
        link.starts_with("https://join.ortus.cc/i#s=") && link.contains("&p="),
        "{link}"
    );
    assert!(made["qr_svg"].as_str().unwrap().starts_with("<?xml"));
    let status = json(
        with(&olena, client.get("/api/v1/me/phone-link"))
            .send()
            .body_text(),
    );
    assert!(status["used_at"].is_null(), "{status}");

    // The app follows it: signed in as Olena, then it adds its key.
    let token = code(link);
    let signed = follow(&token);
    assert_eq!(200, signed.status(), "{}", signed.body_text());
    let signed = json(signed.body_text());
    assert_eq!("olena@ministry.example", signed["email"]);
    assert_eq!("add_sign_in_method", signed["next"]);
    let session = signed["session_token"].as_str().unwrap();
    assert_eq!(
        200,
        with(session, client.get("/api/v1/me")).send().status(),
        "the new session works"
    );
    // The web sees it was used, and by what.
    let status = json(
        with(&olena, client.get("/api/v1/me/phone-link"))
            .send()
            .body_text(),
    );
    assert!(status["used_at"].is_i64(), "{status}");
    assert!(status["agent"].as_str().unwrap().contains("CFNetwork"));

    // Once only.
    let again = follow(&token);
    assert_eq!(410, again.status());
    assert!(again.body_text().contains("phone_link_expired"));
    // Nor a made-up one.
    assert_eq!(410, follow("0123456789abcdef0123456789abcdef").status());

    // A newer link replaces the older one.
    let first = code(
        json(
            with(&olena, client.post("/api/v1/me/phone-link"))
                .body_text("{}")
                .send()
                .body_text(),
        )["link"]
            .as_str()
            .unwrap(),
    );
    let second = code(
        json(
            with(&olena, client.post("/api/v1/me/phone-link"))
                .body_text("{}")
                .send()
                .body_text(),
        )["link"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(410, follow(&first).status(), "replaced");

    // Expired after five minutes.
    ortus_core::runtime::block_on(
        instance
            .store
            .execute("UPDATE core_phone_links SET expires_at = 1", &[]),
    )
    .unwrap();
    assert_eq!(410, follow(&second).status(), "expired");

    // A suspended account's link does not sign in.
    let third = code(
        json(
            with(&olena, client.post("/api/v1/me/phone-link"))
                .body_text("{}")
                .send()
                .body_text(),
        )["link"]
            .as_str()
            .unwrap(),
    );
    let id = ortus_core::runtime::block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        "olena@ministry.example",
    ))
    .unwrap()
    .unwrap()
    .id;
    ortus_core::runtime::block_on(ortus_core::auth::accounts::set(
        instance.store.as_ref(),
        &id,
        ortus_core::auth::accounts::Field::State(ortus_core::auth::accounts::State::Suspended),
    ))
    .unwrap();
    assert_eq!(410, follow(&third).status(), "suspended");
}

#[test]
fn phone_link_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    phone_link((fresh.open)());
}

#[test]
fn phone_link_on_postgres() {
    if let Some(fresh) = common::postgres() {
        phone_link((fresh.open)());
    }
}
