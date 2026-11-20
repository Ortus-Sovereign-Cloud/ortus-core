//! The audit log: a hash chain on both engines that shows any change,
//! removal or cut, filters, and the admin API.

mod common;

use std::sync::Arc;

use ortus_core::audit::{self, Event, Filter, Outcome};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use serde_json::json;

fn chain(store: Arc<dyn AsyncStore>, open: &(dyn Fn() -> Arc<dyn AsyncStore> + Sync)) {
    block_on(ortus_core::storage::install(
        store.as_ref(),
        &ortus_core::schemas(),
        0,
    ))
    .unwrap();
    let s = store.as_ref();
    for n in 0..5 {
        block_on(audit::record(
            s,
            &Event::new("auth.sign_in", Outcome::Done)
                .by("u_olena")
                .from("10.0.0.1")
                .with(json!({"n": n})),
            100 + n,
        ))
        .unwrap();
    }
    block_on(audit::record(
        s,
        &Event::new("drive.share", Outcome::Refused)
            .by("u_marek")
            .on("file-1"),
        200,
    ))
    .unwrap();
    // Writers in other programs at the same time still make one chain.
    let handles: Vec<_> = (0..4)
        .map(|n| {
            let other = open();
            std::thread::spawn(move || {
                for i in 0..5 {
                    block_on(audit::record(
                        other.as_ref(),
                        &Event::new("auth.sign_out", Outcome::Done)
                            .by("u_anna")
                            .with(json!({"w": n, "i": i})),
                        300,
                    ))
                    .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let ok = block_on(audit::verify(s)).unwrap();
    assert_eq!((26, None), (ok.events, ok.broken_at));

    let refused = block_on(audit::list(
        s,
        &Filter {
            action_prefix: Some("drive.".into()),
            limit: 10,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(1, refused.len());
    assert_eq!(
        ("refused", Some("file-1")),
        (refused[0].outcome.as_str(), refused[0].target.as_deref())
    );
    let olena = block_on(audit::list(
        s,
        &Filter {
            actor: Some("u_olena".into()),
            limit: 2,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(vec![5, 4], olena.iter().map(|e| e.seq).collect::<Vec<_>>());
    let older = block_on(audit::list(
        s,
        &Filter {
            actor: Some("u_olena".into()),
            before: Some(4),
            limit: 10,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(3, older.len());

    // A changed event breaks the chain from there.
    block_on(s.execute(
        "UPDATE core_audit SET detail = '{\"n\":9}' WHERE seq = 3",
        &[],
    ))
    .unwrap();
    assert_eq!(Some(3), block_on(audit::verify(s)).unwrap().broken_at);
    block_on(s.execute(
        "UPDATE core_audit SET detail = '{\"n\":2}' WHERE seq = 3",
        &[],
    ))
    .unwrap();
    assert_eq!(None, block_on(audit::verify(s)).unwrap().broken_at);
    // So does a removed one, and one cut from the end.
    block_on(s.execute("DELETE FROM core_audit WHERE seq = 26", &[])).unwrap();
    assert_eq!(Some(26), block_on(audit::verify(s)).unwrap().broken_at);
    block_on(s.execute("DELETE FROM core_audit WHERE seq = 10", &[])).unwrap();
    assert_eq!(Some(10), block_on(audit::verify(s)).unwrap().broken_at);
}

#[test]
fn the_audit_chain_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    chain((fresh.open)(), &*fresh.open);
}

#[test]
fn the_audit_chain_on_postgres() {
    if let Some(fresh) = common::postgres() {
        chain((fresh.open)(), &*fresh.open);
    }
}

#[test]
fn administrators_read_the_log_and_sign_ins_are_in_it() {
    use ortus_core::auth::accounts::Role;
    use rust_web_server::test_client::TestClient;
    let instance = ortus_core::instance::testing::instance(Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    block_on(ortus_core::auth::setup::prepare(
        &instance,
        Some("t"),
        false,
        0,
    ))
    .unwrap();
    let created = client
        .post("/api/v1/setup")
        .header("X-Requested-With", "ortus")
        .body_text(r#"{"token":"t","email":"taras@ministry.example","name":"Taras"}"#)
        .send();
    let owner: serde_json::Value = serde_json::from_str(created.body_text()).unwrap();
    let token = owner["session_token"].as_str().unwrap();
    let member = ortus_core::instance::testing_auth::signed_in(
        &instance,
        "iryna@ministry.example",
        Role::Member,
    );
    assert_eq!(
        403,
        client
            .get("/api/v1/admin/audit")
            .header("Authorization", &format!("Bearer {member}"))
            .send()
            .status()
    );
    let answer = client
        .get("/api/v1/admin/audit?action=auth.")
        .header("Authorization", &format!("Bearer {token}"))
        .send();
    let body: serde_json::Value = serde_json::from_str(answer.body_text()).unwrap();
    let actions: Vec<&str> = body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(
        actions.contains(&"auth.owner_created") && actions.contains(&"auth.sign_in"),
        "{actions:?}"
    );
    let verified = client
        .get("/api/v1/admin/audit/verify")
        .header("Authorization", &format!("Bearer {token}"))
        .send();
    assert!(
        verified.body_text().contains("\"broken_at\":null"),
        "{}",
        verified.body_text()
    );
}
