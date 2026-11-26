//! The phone's way into a web page, on both engines: the signed-in app
//! makes a ticket; the page learns whose it is without using it, then
//! trades it once for a web session; a used or expired ticket does not
//! work; the page to go on to is a path on this site only.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

/// The ticket and the page to go on to, from the address's fragment.
fn parts(url: &str) -> (String, String) {
    let fragment = url.split_once('#').unwrap().1;
    let get = |k: &str| {
        fragment
            .split('&')
            .find_map(|p| p.strip_prefix(&format!("{k}=")))
            .unwrap()
            .to_string()
    };
    (get("t"), get("next"))
}

fn web_ticket(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let make = |next: &str| {
        client
            .post("/api/v1/me/web-ticket")
            .header("Authorization", &format!("Bearer {olena}"))
            .header("X-Requested-With", "ortus")
            .body_text(&serde_json::json!({ "next": next }).to_string())
            .send()
    };
    let post = |path: &str, ticket: &str| {
        client
            .post(path)
            .header("X-Requested-With", "ortus")
            .body_text(&serde_json::json!({ "ticket": ticket }).to_string())
            .send()
    };

    // Only someone signed in makes one.
    assert_eq!(
        401,
        client
            .post("/api/v1/me/web-ticket")
            .header("X-Requested-With", "ortus")
            .body_text("{}")
            .send()
            .status()
    );

    let made = make("/tasks?view=mine");
    assert_eq!(200, made.status(), "{}", made.body_text());
    let url = json(made.body_text())["url"].as_str().unwrap().to_string();
    assert!(
        url.contains("/session/web#t="),
        "the ticket is in the fragment: {url}"
    );
    let (ticket, next) = parts(&url);
    assert_eq!("/tasks%3Fview%3Dmine", next);

    // Whose it is, without using it (as often as the page asks).
    for _ in 0..2 {
        let whose = post("/api/v1/session/web-ticket/whose", &ticket);
        assert_eq!(200, whose.status(), "{}", whose.body_text());
        assert_eq!("olena@ministry.example", json(whose.body_text())["email"]);
    }

    // Traded once for a web session (a cookie); the second time it is gone.
    let traded = post("/api/v1/session/web-ticket", &ticket);
    assert_eq!(200, traded.status(), "{}", traded.body_text());
    assert!(
        traded
            .headers()
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("set-cookie")),
        "a session cookie"
    );
    assert_eq!(
        410,
        post("/api/v1/session/web-ticket", &ticket).status(),
        "used once"
    );
    assert_eq!(
        410,
        post("/api/v1/session/web-ticket/whose", &ticket).status(),
        "nobody's after use"
    );
    assert_eq!(
        410,
        post("/api/v1/session/web-ticket", "not-a-ticket").status()
    );

    // Another site is never the page to go on to.
    for elsewhere in [
        "//evil.example/x",
        "https://evil.example/",
        "/\\evil.example",
    ] {
        let (_, next) = parts(json(make(elsewhere).body_text())["url"].as_str().unwrap());
        assert_eq!("/", next, "{elsewhere}");
    }

    // Expired after a minute.
    let (late, _) = parts(json(make("/").body_text())["url"].as_str().unwrap());
    ortus_core::runtime::block_on(
        instance
            .store
            .execute("UPDATE core_web_tickets SET expires_at = 1", &[]),
    )
    .unwrap();
    assert_eq!(
        410,
        post("/api/v1/session/web-ticket", &late).status(),
        "expired"
    );
}

#[test]
fn web_ticket_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    web_ticket((fresh.open)());
}

#[test]
fn web_ticket_on_postgres() {
    if let Some(fresh) = common::postgres() {
        web_ticket((fresh.open)());
    }
}
