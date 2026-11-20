//! The search index on SQLite and PostgreSQL: words and prefixes in any
//! of the six languages, filters, only what the person may see (their
//! own, and what an app vouches for), removal, and a backup and restore
//! that brings searching back.

mod common;

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store, Instance};
use ortus_core::runtime::block_on;
use ortus_core::search::{self, Entry, Query};
use ortus_core::storage::{AsyncStore, BoxFuture};
use serde_json::json;

/// A stand-in app: Marek may see Olena's "shared" items.
struct Files;

impl App for Files {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "drive",
            group: Group::Create,
            path: "/drive",
            order: 1,
        }
    }
    fn search_owners<'a>(
        &'a self,
        instance: &'a Instance,
        user: &'a str,
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            let olena = ortus_core::auth::accounts::by_email(
                instance.store.as_ref(),
                "olena@ministry.example",
            )
            .await
            .unwrap()
            .unwrap()
            .id;
            if olena == user {
                vec![]
            } else {
                vec![olena]
            }
        })
    }
    fn search_allowed<'a>(
        &'a self,
        _: &'a Instance,
        _: &'a str,
        ids: Vec<String>,
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            ids.into_iter()
                .filter(|i| i.starts_with("shared"))
                .collect()
        })
    }
}

fn words(hits: &[search::Hit]) -> Vec<String> {
    let mut ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
    ids.sort();
    ids
}

fn searching(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(Arc::clone(&store), vec![Arc::new(Files)]);
    signed_in(&instance, "olena@ministry.example", Role::Member);
    signed_in(&instance, "marek@ministry.example", Role::Member);
    let id = |e: &str| {
        block_on(ortus_core::auth::accounts::by_email(store.as_ref(), e))
            .unwrap()
            .unwrap()
            .id
    };
    let (olena, marek) = (id("olena@ministry.example"), id("marek@ministry.example"));
    let s = store.as_ref();
    let add = |app: &str,
               item: &str,
               owner: &str,
               title: &str,
               body: &str,
               meta: serde_json::Value,
               at: i64| {
        block_on(search::index(
            s,
            &Entry {
                app,
                id: item,
                owner,
                title,
                body,
                meta,
                at,
            },
        ))
        .unwrap();
    };
    add(
        "mail",
        "m1",
        &olena,
        "Gauge report and updated shelter agreement",
        "Attached is today's gauge report. Legal should look at clause 4.",
        json!({"from": "Anna Kowalska", "attachment": true}),
        3,
    );
    add(
        "mail",
        "m2",
        &olena,
        "Corrected gauge report (sensor 2)",
        "Now uses sensor 2.",
        json!({"from": "Anna Kowalska"}),
        2,
    );
    add(
        "mail",
        "m3",
        &olena,
        "Police escort schedule",
        "Escort leaves at 07:00.",
        json!({"from": "Jonas Weber"}),
        1,
    );
    add(
        "drive",
        "shared-1",
        &olena,
        "gauge-report.pdf",
        "",
        json!({"size": 212}),
        4,
    );
    add(
        "drive",
        "private-1",
        &olena,
        "gauge-notes.txt",
        "my own gauge notes",
        json!({}),
        5,
    );
    add(
        "calendar",
        "e1",
        &olena,
        "Shelter agreement review with legal",
        "Anna",
        json!({"start": 1}),
        6,
    );
    add(
        "mail",
        "u1",
        &olena,
        "Укриття: звіт про рівень води",
        "Рівень води 4,2 м біля мосту",
        json!({"from": "Олена"}),
        7,
    );
    add(
        "mail",
        "d1",
        &olena,
        "Überflutung gemeldet",
        "Pegelstände steigen",
        json!({}),
        8,
    );

    let find = |user: &str, text: &str| {
        block_on(search::search(&instance, user, &Query::parse(text), 30)).unwrap()
    };
    assert_eq!(
        vec!["m1", "m2", "private-1", "shared-1"],
        words(&find(&olena, "gauge"))
    );
    assert_eq!(vec!["m1", "m2"], words(&find(&olena, "gauge from:anna")));
    assert_eq!(
        vec!["m1"],
        words(&find(&olena, "gaug has:attachment")),
        "prefixes count"
    );
    assert_eq!(vec!["m1", "m2"], words(&find(&olena, "gauge in:mail")));
    assert_eq!(vec!["e1"], words(&find(&olena, "legal in:calendar")));
    assert_eq!(vec!["u1"], words(&find(&olena, "звіт")), "Ukrainian");
    assert_eq!(vec!["d1"], words(&find(&olena, "Überflutung")), "German");
    // Marek: only what the app vouches for.
    assert_eq!(vec!["shared-1"], words(&find(&marek, "gauge")));
    // A result has a snippet around the word.
    let hit = find(&olena, "clause").remove(0);
    assert!(hit.snippet.contains("clause 4"), "{}", hit.snippet);
    // Removed and replaced.
    block_on(search::remove(s, "mail", "m2")).unwrap();
    add(
        "mail",
        "m1",
        &olena,
        "Renamed",
        "nothing about it now",
        json!({}),
        3,
    );
    assert_eq!(vec!["private-1", "shared-1"], words(&find(&olena, "gauge")));
    assert_eq!(vec!["m1"], words(&find(&olena, "renamed")));
}

#[test]
fn search_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    searching((fresh.open)());
}

#[test]
fn search_on_postgres() {
    if let Some(fresh) = common::postgres() {
        searching((fresh.open)());
    }
}

#[test]
fn search_survives_a_backup_and_restore() {
    let from = common::sqlite().unwrap();
    let store = (from.open)();
    let instance = testing_store::instance(Arc::clone(&store), vec![]);
    signed_in(&instance, "olena@ministry.example", Role::Member);
    let olena = block_on(ortus_core::auth::accounts::by_email(
        store.as_ref(),
        "olena@ministry.example",
    ))
    .unwrap()
    .unwrap()
    .id;
    block_on(search::index(
        store.as_ref(),
        &Entry {
            app: "mail",
            id: "m1",
            owner: &olena,
            title: "Gauge report",
            body: "4.2 m",
            meta: json!({}),
            at: 1,
        },
    ))
    .unwrap();
    let dir = common::temp_dir();
    let target = dir.0.join("backup");
    block_on(ortus_core::backup::backup(
        store.as_ref(),
        instance.files.as_ref(),
        &target,
        1,
    ))
    .unwrap();
    let to = common::sqlite().unwrap();
    let fresh = (to.open)();
    let schemas = ortus_core::instance::schemas(&[]);
    let checked = ortus_core::backup::check(&target, &schemas).unwrap();
    let files = ortus_core::files::Files::local(&dir.0.join("files2")).unwrap();
    block_on(ortus_core::backup::restore(
        &checked,
        &schemas,
        fresh.as_ref(),
        &files,
        2,
    ))
    .unwrap();
    let found = block_on(search::find(
        fresh.as_ref(),
        &[olena],
        &Query::parse("gauge"),
        10,
    ))
    .unwrap();
    assert_eq!(1, found.len(), "the full-text index is made again");
}
