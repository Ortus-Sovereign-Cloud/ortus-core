//! Accounts and sessions on both engines, and the checks every API route
//! gets: signed in or not, the role, and changes only from our client.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::{self, Field, NewUser, Role, State};
use ortus_core::auth::sessions::{self, Lifetime};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::Value;

fn installed(store: &Arc<dyn AsyncStore>) {
    block_on(ortus_core::storage::install(
        store.as_ref(),
        &ortus_core::schemas(),
        0,
    ))
    .unwrap();
}

fn accounts_and_sessions(store: Arc<dyn AsyncStore>) {
    installed(&store);
    let s = store.as_ref();
    let olena = block_on(accounts::create(
        s,
        &NewUser {
            email: " Olena@Ministry.Example ",
            name: "Olena Hrytsenko",
            role: Role::Admin,
            state: State::Active,
            expires_at: None,
        },
        10,
    ))
    .unwrap();
    assert_eq!("olena@ministry.example", olena.email);
    assert_eq!(
        Some(olena.clone()),
        block_on(accounts::by_email(s, "OLENA@ministry.example")).unwrap()
    );
    let taken = block_on(accounts::create(
        s,
        &NewUser {
            email: "olena@ministry.example",
            name: "Other",
            role: Role::Member,
            state: State::Active,
            expires_at: None,
        },
        11,
    ));
    assert!(
        matches!(taken, Err(ortus_core::storage::StoreError::Conflict(_))),
        "{taken:?}"
    );

    let life = Lifetime {
        idle_ms: 1_000_000,
        max_ms: 5_000_000,
    };
    let token = block_on(sessions::start(
        s, &olena.id, "passkey", "10.0.0.1", "Firefox", life, 100,
    ))
    .unwrap();
    let session = block_on(sessions::find(s, &token, life, 200))
        .unwrap()
        .unwrap();
    assert_eq!(olena.id, session.user.id);
    assert_eq!("passkey", session.methods);
    // Only the hash is stored.
    let stored = block_on(s.query("SELECT token_hash FROM core_sessions", &[])).unwrap();
    assert_ne!(Some(token.as_str()), stored[0].text(0));
    // Idle expiry moves on with use, but never past the maximum.
    assert!(block_on(sessions::find(s, &token, life, 900_000))
        .unwrap()
        .is_some());
    assert!(block_on(sessions::find(s, &token, life, 1_800_000))
        .unwrap()
        .is_some());
    assert!(block_on(sessions::find(s, &token, life, 5_000_200))
        .unwrap()
        .is_none());

    // A suspended account's sessions no longer work.
    let token = block_on(sessions::start(s, &olena.id, "passkey", "", "", life, 300)).unwrap();
    block_on(accounts::set(s, &olena.id, Field::State(State::Suspended))).unwrap();
    assert!(block_on(sessions::find(s, &token, life, 400))
        .unwrap()
        .is_none());
    block_on(accounts::set(s, &olena.id, Field::State(State::Active))).unwrap();
    assert!(block_on(sessions::find(s, &token, life, 400))
        .unwrap()
        .is_some());
    assert_eq!(1, block_on(sessions::list(s, &olena.id)).unwrap().len());
    assert_eq!(1, block_on(sessions::end_all(s, &olena.id)).unwrap());
    assert!(block_on(sessions::find(s, &token, life, 400))
        .unwrap()
        .is_none());
    assert!(block_on(sessions::find(s, "made-up", life, 400))
        .unwrap()
        .is_none());
}

#[test]
fn accounts_and_sessions_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    accounts_and_sessions((fresh.open)());
}

#[test]
fn accounts_and_sessions_on_postgres() {
    if let Some(fresh) = common::postgres() {
        accounts_and_sessions((fresh.open)());
    }
}

#[test]
fn email_addresses_are_checked() {
    assert_eq!(
        Some("a@b.example".to_string()),
        accounts::normal_email(" A@B.example ")
    );
    for bad in [
        "",
        "a",
        "a@b",
        "@b.example",
        "a@.example",
        "a b@c.example",
        "a@b.example,c@d.example",
        "a@b_c.example",
    ] {
        assert_eq!(None, accounts::normal_email(bad), "{bad}");
    }
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn api_routes_check_who_is_asking() {
    let instance = ortus_core::instance::testing::instance(Vec::new());
    let s = instance.store.as_ref();
    let member = block_on(accounts::create(
        s,
        &NewUser {
            email: "iryna@ministry.example",
            name: "Iryna",
            role: Role::Member,
            state: State::Active,
            expires_at: None,
        },
        1,
    ))
    .unwrap();
    let token = block_on(sessions::start(
        s,
        &member.id,
        "passkey",
        "",
        "",
        Lifetime::default(),
        ortus_core::program::now_ms(),
    ))
    .unwrap();
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));

    let anonymous = client.get("/api/v1/me").send();
    assert_eq!(401, anonymous.status());
    assert_eq!(
        "unauthenticated",
        json(anonymous.body_text())["error"]["code"]
    );
    assert_eq!(401, client.get("/api/v1/apps").send().status());
    assert_eq!(200, client.get("/api/v1/health").send().status());

    let me = client
        .get("/api/v1/me")
        .header("Authorization", &format!("Bearer {token}"))
        .send();
    assert_eq!(200, me.status(), "{}", me.body_text());
    assert_eq!("iryna@ministry.example", json(me.body_text())["email"]);
    let by_cookie = client
        .get("/api/v1/me")
        .header("Cookie", &format!("other=1; ortus={token}"))
        .send();
    assert_eq!(200, by_cookie.status());

    // A change without our header is refused, signed in or not.
    let forged = client
        .post("/api/v1/session/sign-out")
        .header("Cookie", &format!("ortus={token}"))
        .send();
    assert_eq!(403, forged.status());
    assert_eq!("not_from_client", json(forged.body_text())["error"]["code"]);
    assert_eq!(
        200,
        client
            .get("/api/v1/me")
            .header("Cookie", &format!("ortus={token}"))
            .send()
            .status()
    );

    let out = client
        .post("/api/v1/session/sign-out")
        .header("Cookie", &format!("ortus={token}"))
        .header("X-Requested-With", "ortus")
        .send();
    assert_eq!(200, out.status());
    let cookie = out.header("Set-Cookie").unwrap();
    assert!(
        cookie.starts_with("ortus=;")
            && cookie.contains("Max-Age=0")
            && cookie.contains("HttpOnly"),
        "{cookie}"
    );
    assert_eq!(
        401,
        client
            .get("/api/v1/me")
            .header("Cookie", &format!("ortus={token}"))
            .send()
            .status()
    );
}

#[test]
fn the_first_owner_is_created_once_with_the_setup_link() {
    let instance = ortus_core::instance::testing::instance(Vec::new());
    let link = block_on(ortus_core::auth::setup::prepare(&instance, None, false, 1))
        .unwrap()
        .unwrap();
    let token = link.rsplit_once('#').unwrap().1.to_string();
    assert!(link.starts_with(&format!("{}/setup#", instance.config.public_url)));
    // A second start does not replace the printed link; the command does.
    assert_eq!(
        None,
        block_on(ortus_core::auth::setup::prepare(&instance, None, false, 2)).unwrap()
    );

    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    assert_eq!(
        true,
        json(client.get("/api/v1/setup").send().body_text())["needed"]
    );
    let post = |token: &str| {
        client
            .post("/api/v1/setup")
            .header("X-Requested-With", "ortus")
            .header("Content-Type", "application/json")
            .body_text(&format!(
                r#"{{"token":"{token}","email":"taras@ministry.example","name":"Taras Bondar"}}"#
            ))
            .send()
    };
    let wrong = post("not-the-token");
    assert_eq!(403, wrong.status());
    assert_eq!("bad_setup_token", json(wrong.body_text())["error"]["code"]);

    let created = post(&token);
    assert_eq!(200, created.status(), "{}", created.body_text());
    let body = json(created.body_text());
    assert_eq!("owner", body["role"]);
    let session = body["session_token"].as_str().unwrap().to_string();
    assert!(created.header("Set-Cookie").unwrap().contains(&session));
    let me = client
        .get("/api/v1/me")
        .header("Authorization", &format!("Bearer {session}"))
        .send();
    assert_eq!("taras@ministry.example", json(me.body_text())["email"]);

    assert_eq!(
        false,
        json(client.get("/api/v1/setup").send().body_text())["needed"]
    );
    assert_eq!(409, post(&token).status());
    assert_eq!(
        None,
        block_on(ortus_core::auth::setup::prepare(&instance, None, true, 3)).unwrap()
    );
}
