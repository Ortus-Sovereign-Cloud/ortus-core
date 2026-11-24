//! The functions apps read people, groups, quotas, notifications and the
//! search index through, on both engines.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::{self, NewUser, Role, State};
use ortus_core::notify::{self, Kind, Notice};
use ortus_core::people;
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;

fn reading(store: Arc<dyn AsyncStore>) {
    let instance = ortus_core::instance::testing_store::instance(Arc::clone(&store), vec![]);
    let s = instance.store.as_ref();
    let add = |email: &str, name: &str, state: State| {
        block_on(accounts::create(
            s,
            &NewUser {
                email,
                name,
                role: Role::Member,
                state,
                expires_at: None,
            },
            1,
        ))
        .unwrap()
        .id
    };
    let olena = add("olena@ministry.example", "Olena Hrytsenko", State::Active);
    let marek = add("marek@ministry.example", "Marek Nowak", State::Invited);
    add("new@ministry.example", "Not Yet", State::Unverified);
    block_on(ortus_core::auth::admin_users::join_group(
        s,
        "Flood team",
        &olena,
    ))
    .unwrap();
    block_on(ortus_core::auth::admin_users::join_group(
        s,
        "Flood team",
        &marek,
    ))
    .unwrap();
    block_on(ortus_core::auth::admin_users::join_group(
        s, "Comms", &olena,
    ))
    .unwrap();

    // Finding colleagues: by name or address, invited only when asked, and
    // a LIKE wildcard typed is left out.
    let found = block_on(people::find(s, "ministry", None, true, 10)).unwrap();
    assert_eq!(
        vec!["Marek Nowak", "Olena Hrytsenko"],
        found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(
        1,
        block_on(people::find(s, "", None, false, 10))
            .unwrap()
            .len()
    );
    let wild = block_on(people::find(s, "Ole%na", None, true, 10)).unwrap();
    assert_eq!(
        vec!["Olena Hrytsenko"],
        wild.iter().map(|p| p.name.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(2, block_on(people::count(s, true)).unwrap());

    // Groups: of a person, one by id, all with counts, memberships.
    let mine = block_on(people::groups_of(s, &olena)).unwrap();
    assert_eq!(
        vec!["Comms", "Flood team"],
        mine.iter().map(|g| g.name.as_str()).collect::<Vec<_>>()
    );
    let flood = mine
        .iter()
        .find(|g| g.name == "Flood team")
        .unwrap()
        .id
        .clone();
    assert_eq!(
        "Flood team",
        block_on(people::group(s, &flood)).unwrap().unwrap().name
    );
    assert_eq!(None, block_on(people::group(s, "nope")).unwrap());
    let all = block_on(people::groups(s)).unwrap();
    assert_eq!(2, all.iter().find(|g| g.id == flood).unwrap().members);
    let members = block_on(people::find(s, "", Some(&flood), true, 10)).unwrap();
    assert_eq!(2, members.len());
    assert_eq!(
        Some(&vec!["Comms".to_string(), "Flood team".to_string()]),
        block_on(people::memberships(s)).unwrap().get(&olena)
    );

    // An address: the account, unless it is still confirming.
    assert_eq!(
        Some(olena.clone()),
        block_on(people::id_by_email(s, " Olena@Ministry.example ")).unwrap()
    );
    assert_eq!(
        None,
        block_on(people::id_by_email(s, "new@ministry.example")).unwrap()
    );

    // Quotas: none until set; locked inside a transaction.
    assert_eq!(None, block_on(people::quota(s, &olena)).unwrap());
    block_on(s.execute(
        "UPDATE core_users SET quota_bytes = 1000 WHERE id = $1",
        &[olena.as_str().into()],
    ))
    .unwrap();
    assert_eq!(Some(1000), block_on(people::quota(s, &olena)).unwrap());
    let mut tx = block_on(s.begin()).unwrap();
    assert_eq!(
        Some(1000),
        block_on(people::quota_locked(&mut *tx, &olena)).unwrap()
    );
    block_on(tx.commit()).unwrap();

    // Recent notifications of one kind.
    for (kind, title) in [
        (Kind::Alert, "River at 4.2 m"),
        (Kind::Mention, "Marek mentioned you"),
    ] {
        block_on(notify::add(
            s,
            &olena,
            &Notice {
                app: "test",
                kind,
                title,
                detail: "",
                path: "/x",
                key: None,
                args: serde_json::Value::Null,
            },
        ))
        .unwrap();
    }
    let alerts = block_on(notify::recent(s, &olena, Kind::Alert, 0, 5)).unwrap();
    assert_eq!(
        vec!["River at 4.2 m"],
        alerts.iter().map(|n| n.title.as_str()).collect::<Vec<_>>()
    );
    assert!(
        block_on(notify::recent(s, &olena, Kind::Alert, i64::MAX, 5))
            .unwrap()
            .is_empty()
    );

    // The search index: which items are missing, and moving owners.
    block_on(ortus_core::search::index(
        s,
        &ortus_core::search::Entry {
            app: "test",
            id: "a",
            owner: "user:1",
            title: "Gauge",
            body: "",
            meta: serde_json::json!({"path": "/a"}),
            at: 1,
        },
    ))
    .unwrap();
    let ids = vec!["a".to_string(), "b".to_string()];
    assert_eq!(
        vec!["b".to_string()],
        block_on(ortus_core::search::missing(s, "test", &ids)).unwrap()
    );
    let mut tx = block_on(s.begin()).unwrap();
    block_on(ortus_core::search::set_owner_in(
        &mut *tx, "test", "user:1", "list:7", &ids,
    ))
    .unwrap();
    block_on(tx.commit()).unwrap();
    let owner = block_on(s.query("SELECT owner FROM core_search WHERE id = 'a'", &[])).unwrap();
    assert_eq!(Some("list:7"), owner[0].text(0));
}

#[test]
fn reading_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    reading((fresh.open)());
}

#[test]
fn reading_on_postgres() {
    if let Some(fresh) = common::postgres() {
        reading((fresh.open)());
    }
}
