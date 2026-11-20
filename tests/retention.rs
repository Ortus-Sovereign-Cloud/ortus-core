//! Retention periods and legal holds, on both engines: the daily sweep
//! hands an app the rules, and nothing on hold goes.

mod common;

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::program::{now_ms, AppWorker};
use ortus_core::retention::Rules;
use ortus_core::runtime::block_on;
use ortus_core::storage::{AsyncStore, AsyncTx, BoxFuture, Migration};
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

const DAY: i64 = 24 * 60 * 60 * 1000;

/// Keeps chat messages, each from a person in a room.
struct Talk;

const TALK: &[Migration] = &[Migration {
    version: 1,
    name: "messages",
    sql: "CREATE TABLE talk_messages (id TEXT PRIMARY KEY, author TEXT NOT NULL, room TEXT NOT NULL, created_at BIGINT NOT NULL)",
}];

impl App for Talk {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "talk",
            group: Group::Communicate,
            path: "/talk",
            order: 1,
        }
    }
    fn migrations(&self) -> &'static [Migration] {
        TALK
    }
    fn purge<'a>(
        &'a self,
        tx: &'a mut dyn AsyncTx,
        rules: &'a Rules,
    ) -> BoxFuture<'a, Result<u64, String>> {
        Box::pin(async move {
            let rows = tx
                .query(
                    "SELECT id, author, room, created_at FROM talk_messages",
                    &[],
                )
                .await
                .map_err(|e| e.to_string())?;
            let mut gone = 0;
            for r in rows {
                let (id, author, room) = (
                    r.text(0).unwrap_or_default().to_string(),
                    r.text(1).unwrap_or_default(),
                    r.text(2).unwrap_or_default(),
                );
                if rules.may_purge("chat", r.int(3).unwrap_or_default(), &[author], Some(room)) {
                    tx.execute(
                        "DELETE FROM talk_messages WHERE id = $1",
                        &[id.as_str().into()],
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    gone += 1;
                }
            }
            Ok(gone)
        })
    }
}

fn retention(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, vec![Arc::new(Talk)]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let admin = signed_in(&instance, "marek@ministry.example", Role::Admin);
    signed_in(&instance, "olena@ministry.example", Role::Member);
    let id_of = |email: &str| {
        block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            email,
        ))
        .unwrap()
        .unwrap()
        .id
    };
    let (olena, marek) = (
        id_of("olena@ministry.example"),
        id_of("marek@ministry.example"),
    );
    let send = |method: &str, path: &str, token: &str, body: Value| {
        let request = match method {
            "GET" => client.get(path),
            "PUT" => client.put(path),
            _ => client.post(path),
        };
        request
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .body_text(&body.to_string())
            .send()
    };
    let read = |method: &str, path: &str, token: &str, body: Value| -> Value {
        let response = send(method, path, token, body);
        assert_eq!(
            200,
            response.status(),
            "{method} {path}: {}",
            response.body_text()
        );
        serde_json::from_str(response.body_text()).unwrap()
    };

    let shown = read("GET", "/api/v1/admin/retention", &admin, Value::Null);
    assert_eq!(
        json!([{"kind": "mail", "days": 3650}, {"kind": "chat", "days": 2555}, {"kind": "recordings", "days": 365}, {"kind": "files", "days": null}, {"kind": "deleted", "days": 90}]),
        shown["periods"]
    );
    assert_eq!(
        403,
        send(
            "PUT",
            "/api/v1/admin/retention/chat",
            &admin,
            json!({"days": 30})
        )
        .status(),
        "owners only"
    );
    assert_eq!(
        400,
        send(
            "PUT",
            "/api/v1/admin/retention/chat",
            &owner,
            json!({"days": 0})
        )
        .status()
    );
    read(
        "PUT",
        "/api/v1/admin/retention/chat",
        &owner,
        json!({"days": 30}),
    );

    // Old messages: Olena's in the logistics room, Marek's in two rooms.
    let old = now_ms() - 40 * DAY;
    for (id, author, room, at) in [
        ("a", &olena, "talk:room:logistics", old),
        ("b", &marek, "talk:room:logistics", old),
        ("c", &marek, "talk:room:board", old),
        ("d", &marek, "talk:room:board", now_ms()),
    ] {
        block_on(instance.store.execute(
            "INSERT INTO talk_messages (id, author, room, created_at) VALUES ($1, $2, $3, $4)",
            &[id.into(), author.as_str().into(), room.into(), at.into()],
        ))
        .unwrap();
    }
    // A hold on Olena, and one on the board room; then released, the board hold.
    let held = read(
        "POST",
        "/api/v1/admin/holds",
        &owner,
        json!({"case_ref": "2026-114", "people": ["olena@ministry.example"]}),
    );
    assert_eq!("2026-114", held["holds"][0]["case_ref"]);
    let board = read(
        "POST",
        "/api/v1/admin/holds",
        &owner,
        json!({"case_ref": "2026-115", "channels": ["talk:room:board"]}),
    );
    let board_hold = board["holds"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(
        400,
        send(
            "POST",
            "/api/v1/admin/holds",
            &owner,
            json!({"case_ref": "x", "people": ["nobody@ministry.example"]})
        )
        .status()
    );

    let sweep = || {
        let worker = AppWorker::new(Arc::clone(&instance), "test");
        while block_on(worker.tick(now_ms())).unwrap() {}
        let ids: Vec<String> = block_on(
            instance
                .store
                .query("SELECT id FROM talk_messages ORDER BY id", &[]),
        )
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
        ids
    };
    // Olena's message is held through her; the board's through the room.
    assert_eq!(vec!["a", "c", "d"], sweep());
    let purged = read(
        "GET",
        "/api/v1/admin/audit?action=retention.purged",
        &owner,
        Value::Null,
    );
    assert_eq!(
        json!({"talk": 1}),
        serde_json::from_str::<Value>(purged["events"][0]["detail"].as_str().unwrap()).unwrap()
    );

    // Released, the board's old message goes at the next sweep.
    read(
        "POST",
        &format!("/api/v1/admin/holds/{board_hold}/release"),
        &owner,
        Value::Null,
    );
    assert_eq!(
        404,
        send(
            "POST",
            &format!("/api/v1/admin/holds/{board_hold}/release"),
            &owner,
            Value::Null
        )
        .status()
    );
    block_on(instance.store.execute("DELETE FROM ortus_jobs", &[])).unwrap();
    assert_eq!(vec!["a", "d"], sweep());

    // A held person's expired account is not deleted either.
    block_on(instance.store.execute(
        "UPDATE core_users SET expires_at = 1 WHERE id = $1",
        &[olena.as_str().into()],
    ))
    .unwrap();
    sweep();
    assert!(block_on(ortus_core::auth::accounts::by_id(
        instance.store.as_ref(),
        &olena
    ))
    .unwrap()
    .is_some());
}

#[test]
fn retention_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    retention((fresh.open)());
}

#[test]
fn retention_on_postgres() {
    if let Some(fresh) = common::postgres() {
        retention((fresh.open)());
    }
}
