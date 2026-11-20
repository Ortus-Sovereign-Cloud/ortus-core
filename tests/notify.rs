//! Notifications on SQLite and PostgreSQL: apps add them, a person sees
//! their own newest first with unread counts by kind, marks them read,
//! sets quiet hours and do not disturb; only the newest 500 are kept, and
//! paths stay inside Ortus.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::notify::{self, Kind, Notice};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

fn call<A: rust_web_server::application::Application>(
    client: &TestClient<A>,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (i16, Value) {
    let url = format!("/api/v1{path}");
    let mut r = match method {
        "GET" => client.get(&url),
        "PATCH" => client.patch(&url),
        _ => client.post(&url),
    };
    r = r
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus");
    if let Some(b) = body {
        r = r
            .header("Content-Type", "application/json")
            .body_bytes(b.to_string().into_bytes());
    }
    let response = r.send();
    (
        response.status(),
        serde_json::from_slice(response.body_bytes()).unwrap_or(Value::Null),
    )
}

fn notifying(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(Arc::clone(&store), vec![]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let marek = signed_in(&instance, "marek@ministry.example", Role::Member);
    let id = |e: &str| {
        block_on(ortus_core::auth::accounts::by_email(store.as_ref(), e))
            .unwrap()
            .unwrap()
            .id
    };
    let olena_id = id("olena@ministry.example");

    for (kind, title, path) in [
        (
            Kind::Mention,
            "Anna mentioned you in #flood-response",
            "/chat?room=r1",
        ),
        (
            Kind::Approval,
            "Review: Flood response plan v12",
            "/approvals",
        ),
        (
            Kind::Alert,
            "Gauge at the bridge passed 4.2 m",
            "/chat?room=r2",
        ),
        (Kind::Update, "A path out", "https://evil.example/"),
    ] {
        block_on(notify::add(
            store.as_ref(),
            &olena_id,
            &Notice {
                app: "chat",
                kind,
                title,
                detail: "now",
                path,
                key: None,
                args: json!({}),
            },
        ))
        .unwrap();
    }
    let (status, mine) = call(&client, &olena, "GET", "/notifications", None);
    assert_eq!(200, status);
    assert_eq!(4, mine["unread"]);
    assert_eq!(
        json!({"mention": 1, "approval": 1, "alert": 1, "update": 1}),
        mine["counts"]
    );
    let items = mine["items"].as_array().unwrap();
    assert_eq!(4, items.len());
    assert_eq!(
        "/",
        items.iter().find(|i| i["title"] == "A path out").unwrap()["path"],
        "paths stay inside"
    );
    // Someone else's are not seen.
    assert_eq!(
        0,
        call(&client, &marek, "GET", "/notifications", None).1["items"]
            .as_array()
            .unwrap()
            .len()
    );

    let alert = items.iter().find(|i| i["kind"] == "alert").unwrap()["id"].clone();
    // Marek cannot mark hers.
    call(
        &client,
        &marek,
        "POST",
        "/notifications/read",
        Some(json!({"ids": [alert]})),
    );
    let (_, after) = call(
        &client,
        &olena,
        "POST",
        "/notifications/read",
        Some(json!({"ids": [alert]})),
    );
    assert_eq!(3, after["unread"]);
    assert_eq!(0, after["counts"]["alert"]);
    let (_, all) = call(
        &client,
        &olena,
        "POST",
        "/notifications/read",
        Some(json!({"all": true})),
    );
    assert_eq!(0, all["unread"]);

    // Quiet hours and do not disturb.
    assert_eq!(
        400,
        call(
            &client,
            &olena,
            "PATCH",
            "/notifications/settings",
            Some(json!({"quietFrom": "25:00", "quietTo": "08:00"}))
        )
        .0
    );
    let (_, set) = call(
        &client,
        &olena,
        "PATCH",
        "/notifications/settings",
        Some(json!({"quietFrom": "17:00", "quietTo": "08:00"})),
    );
    assert_eq!(
        json!({"from": "17:00", "to": "08:00", "dnd": false}),
        set["quiet"]
    );
    assert!(
        block_on(notify::quiet(store.as_ref(), &olena_id, 22 * 60)).unwrap(),
        "night is quiet"
    );
    assert!(
        !block_on(notify::quiet(store.as_ref(), &olena_id, 12 * 60)).unwrap(),
        "noon is not"
    );
    call(
        &client,
        &olena,
        "PATCH",
        "/notifications/settings",
        Some(json!({"dnd": true})),
    );
    assert!(
        block_on(notify::quiet(store.as_ref(), &olena_id, 12 * 60)).unwrap(),
        "do not disturb"
    );
    // Do not disturb off again: the quiet hours set before are still there.
    call(
        &client,
        &olena,
        "PATCH",
        "/notifications/settings",
        Some(json!({"dnd": false})),
    );
    assert!(
        block_on(notify::quiet(store.as_ref(), &olena_id, 22 * 60)).unwrap(),
        "quiet hours kept"
    );
    // `null` clears them.
    call(
        &client,
        &olena,
        "PATCH",
        "/notifications/settings",
        Some(json!({"quietFrom": null, "quietTo": null})),
    );
    assert!(!block_on(notify::quiet(store.as_ref(), &olena_id, 22 * 60)).unwrap());

    // Only the newest 500 are kept.
    let marek_id = id("marek@ministry.example");
    for n in 0..505 {
        block_on(notify::add(
            store.as_ref(),
            &marek_id,
            &Notice {
                app: "tasks",
                kind: Kind::Update,
                title: &format!("n{n}"),
                detail: "",
                path: "/tasks",
                key: None,
                args: json!({}),
            },
        ))
        .unwrap();
    }
    let kept = block_on(store.query(
        "SELECT COUNT(*) FROM core_notifications WHERE user_id = $1",
        &[marek_id.as_str().into()],
    ))
    .unwrap();
    assert!(kept[0].int(0).unwrap() <= 500 && kept[0].int(0).unwrap() >= 495);
    // A path that would leave Ortus becomes "/".
    for bad in ["/\\evil.example", "/\t/evil.example", "//evil.example"] {
        block_on(notify::add(
            store.as_ref(),
            &olena_id,
            &Notice {
                app: "chat",
                kind: Kind::Alert,
                title: "x",
                detail: "",
                path: bad,
                key: None,
                args: json!({}),
            },
        ))
        .unwrap();
    }
    let paths = block_on(store.query(
        "SELECT path FROM core_notifications WHERE user_id = $1 AND title = 'x'",
        &[olena_id.as_str().into()],
    ))
    .unwrap();
    assert!(paths.iter().all(|r| r.text(0) == Some("/")));
}

#[test]
fn notifications_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    notifying((fresh.open)());
}

#[test]
fn notifications_on_postgres() {
    if let Some(fresh) = common::postgres() {
        notifying((fresh.open)());
    }
}
