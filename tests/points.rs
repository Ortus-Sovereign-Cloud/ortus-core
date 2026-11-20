//! Restore points on SQLite and PostgreSQL: a point holds every table and
//! file; a file's contents are stored once however many points name it;
//! old points thin out (hourly, daily, weekly) and contents no point names
//! go; a point exports to a backup that restores into a fresh
//! installation with the same rows and files.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::files::Files;
use ortus_core::instance::{schemas, testing_auth::signed_in, testing_store};
use ortus_core::points;
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn pointing(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(Arc::clone(&store), vec![]);
    signed_in(&instance, "olena@ministry.example", Role::Owner);
    signed_in(&instance, "marek@ministry.example", Role::Member);
    block_on(instance.files.put("drive/a.txt", b"shelter list")).unwrap();
    block_on(instance.files.put("drive/b.txt", b"shelter list")).unwrap();
    let dir = common::temp_dir();
    let dest = Files::local(&dir.0.join("backups")).unwrap();

    let now = 1_795_200_000_000i64;
    let first = block_on(points::take(&instance, &dest, now - 3 * DAY, true)).unwrap();
    assert_eq!(2, first.files);
    // Two files with the same contents: one blob.
    assert_eq!(1, block_on(dest.list("blobs/")).unwrap().len());
    block_on(instance.files.put("drive/c.txt", b"bus timetable")).unwrap();
    let second = block_on(points::take(&instance, &dest, now - 2 * DAY, false)).unwrap();
    assert_eq!(3, second.files);
    assert_eq!(2, block_on(dest.list("blobs/")).unwrap().len());
    // Hourly points for the last three hours.
    for h in [3, 2, 1] {
        block_on(points::take(&instance, &dest, now - h * HOUR, false)).unwrap();
    }
    assert_eq!(
        5,
        block_on(store.query("SELECT COUNT(*) FROM core_points", &[])).unwrap()[0]
            .int(0)
            .unwrap()
    );

    // Thinning: three days ago (daily) and the hourly ones stay; nothing
    // is older than a day's bucket, so all five are kept.
    assert_eq!(0, block_on(points::prune(&instance, &dest, now)).unwrap());
    // A year and a day later only the newest is kept, and the contents of
    // the file only the dropped points named go.
    block_on(instance.files.delete("drive/a.txt")).unwrap();
    block_on(instance.files.delete("drive/b.txt")).unwrap();
    let later = now + 366 * DAY;
    block_on(points::take(&instance, &dest, later, true)).unwrap();
    assert_eq!(5, block_on(points::prune(&instance, &dest, later)).unwrap());
    assert_eq!(
        1,
        block_on(dest.list("blobs/")).unwrap().len(),
        "only the bus timetable is named still"
    );

    // The point exports to a backup that restores into a fresh install.
    let name = points::name_at(later);
    let out = dir.0.join("export");
    let manifest = block_on(points::export(&dest, &name, &out)).unwrap();
    assert_eq!(1, manifest.files.count);
    let all = schemas(&[]);
    let checked = ortus_core::backup::check(&out, &all).unwrap();
    let fresh = common::sqlite().unwrap();
    let target = (fresh.open)();
    let files_dir = common::temp_dir();
    let target_files = Files::local(&files_dir.0).unwrap();
    block_on(ortus_core::backup::restore(
        &checked,
        &all,
        target.as_ref(),
        &target_files,
        later,
    ))
    .unwrap();
    let users = |s: &dyn AsyncStore| {
        block_on(s.query("SELECT COUNT(*) FROM core_users", &[])).unwrap()[0]
            .int(0)
            .unwrap()
    };
    assert_eq!(users(store.as_ref()), users(target.as_ref()));
    // The restore test does the same into a scratch installation, checks
    // rows, files and keys, and records it.
    let tested = block_on(points::test_restore(&instance, &dest, &name));
    assert!(tested.ok, "{tested:?}");
    assert!(tested.detail.ends_with("keys open"), "{tested:?}");
    let recorded = block_on(store.query("SELECT ok FROM core_restore_tests", &[])).unwrap();
    assert_eq!(Some(1), recorded.last().and_then(|r| r.int(0)));
    // A point changed in the backup store is refused (its sealed checksum
    // no longer matches), and so is one sealed by another installation.
    block_on(points::verify(&dest, &name, &instance.keys)).unwrap();
    let key = format!("points/{name}/manifest.json");
    let text = String::from_utf8(block_on(dest.get(&key)).unwrap().unwrap()).unwrap();
    block_on(dest.put(&key, text.replace("\"files\"", "\"files\" ").as_bytes())).unwrap();
    assert!(block_on(points::verify(&dest, &name, &instance.keys)).is_err());
    block_on(dest.put(&key, text.as_bytes())).unwrap();
    let other = ortus_core::secrets::Keyring::load(
        std::path::Path::new("/nonexistent"),
        Some("CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk=".into()),
    )
    .unwrap();
    assert!(block_on(points::verify(&dest, &name, &other)).is_err());
    assert!(!block_on(points::test_restore(&instance, &dest, "../x")).ok);
    assert_eq!(
        Some(b"bus timetable".to_vec()),
        block_on(target_files.get("drive/c.txt")).unwrap()
    );
}

#[test]
fn which_points_are_kept() {
    let now = 400 * DAY;
    // A point every hour for three weeks.
    let times: Vec<i64> = (0..21 * 24).map(|h| now - h * HOUR).collect();
    let kept = points::keep(&times, now);
    // Every one of the last 48 hours (0 to 48 hours back) and, older,
    // the newest of each day.
    assert!(kept.contains(&now));
    assert!(kept.contains(&(now - 48 * HOUR)));
    let older: std::collections::BTreeSet<i64> = kept
        .iter()
        .filter(|t| now - **t > 48 * HOUR)
        .map(|t| t.div_euclid(DAY))
        .collect();
    assert_eq!(
        older.len(),
        kept.iter().filter(|t| now - **t > 48 * HOUR).count(),
        "one a day"
    );
    assert_eq!(68, kept.len(), "{kept:?}");
    // A year and more back: nothing but the newest of all.
    assert_eq!(
        1,
        points::keep(&[now - 400 * DAY, now - 380 * DAY], now).len()
    );
}

#[test]
fn names_read_as_times() {
    assert_eq!("20261120-180000", points::name_at(1_795_197_600_000));
}

#[test]
fn points_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    pointing((fresh.open)());
}

#[test]
fn points_on_postgres() {
    if let Some(fresh) = common::postgres() {
        pointing((fresh.open)());
    }
}

/// A stand-in app with lists to bring back.
struct Notes;

const NOTES: &[ortus_core::storage::Migration] = &[ortus_core::storage::Migration {
    version: 1,
    name: "lists",
    sql:
        "CREATE TABLE notes_lists (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL)",
}];

impl ortus_core::app::App for Notes {
    fn info(&self) -> ortus_core::app::AppInfo {
        ortus_core::app::AppInfo {
            id: "notes",
            group: ortus_core::app::Group::Organise,
            path: "/notes",
            order: 9,
        }
    }
    fn migrations(&self) -> &'static [ortus_core::storage::Migration] {
        NOTES
    }
    fn restorable(&self) -> &'static [&'static str] {
        &["list"]
    }
    fn restore_choices<'a>(
        &'a self,
        point: &'a points::Snapshot,
        user: &'a str,
        _what: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<Vec<(String, String)>, String>> {
        Box::pin(async move {
            Ok(point
                .rows("notes_lists", |r| points::text(r, "user_id") == user)
                .await?
                .iter()
                .map(|r| {
                    (
                        points::text(r, "id").to_string(),
                        points::text(r, "name").to_string(),
                    )
                })
                .collect())
        })
    }
    fn restore<'a>(
        &'a self,
        instance: &'a Arc<ortus_core::instance::Instance>,
        _point: &'a points::Snapshot,
        user: &'a str,
        _what: &'a str,
        _id: &'a str,
        into: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            instance
                .store
                .execute(
                    "INSERT INTO notes_lists (id, user_id, name) VALUES ($1, $2, $3)",
                    &["restored".into(), user.into(), into.into()],
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok("/notes?list=restored".into())
        })
    }
}

fn call<A: rust_web_server::application::Application>(
    client: &rust_web_server::test_client::TestClient<A>,
    token: &str,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (i16, serde_json::Value) {
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
        serde_json::from_slice(response.body_bytes()).unwrap_or(serde_json::Value::Null),
    )
}

/// An administrator brings back a person's list as it was into a new one
/// beside theirs; a person may only when self-restore is on, and only
/// their own.
#[test]
fn restoring_one_thing_into_a_new_place() {
    let dir = common::temp_dir();
    std::env::set_var("ORTUS_BACKUP_DIR", dir.0.join("backups"));
    let sqlite = common::sqlite().unwrap();
    let pg = common::postgres();
    let mut stores = vec![("sqlite", (sqlite.open)())];
    if let Some(pg) = &pg {
        stores.push(("postgres", (pg.open)()));
    }
    for (name, store) in stores {
        let instance = testing_store::instance(Arc::clone(&store), vec![Arc::new(Notes)]);
        let client =
            rust_web_server::test_client::TestClient::new(ortus_core::app(Arc::clone(&instance)));
        let owner = signed_in(&instance, "owner@ministry.example", Role::Owner);
        let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
        let olena_id = block_on(ortus_core::auth::accounts::by_email(
            store.as_ref(),
            "olena@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        block_on(store.execute(
            "INSERT INTO notes_lists (id, user_id, name) VALUES ('l1', $1, 'Agreements')",
            &[olena_id.as_str().into()],
        ))
        .unwrap();
        let (status, taken) = call(&client, &owner, "POST", "/admin/backup/points", None);
        assert_eq!(200, status, "{name}: {taken}");
        let point = taken["points"][0]["name"].as_str().unwrap().to_string();
        assert_eq!(
            serde_json::json!([{"app": "notes", "what": "list"}]),
            taken["restorable"],
            "{name}"
        );
        // Gone since.
        block_on(store.execute("DELETE FROM notes_lists", &[])).unwrap();

        let ask = format!(
            "/backup/choices?point={point}&app=notes&what=list&user=olena%40ministry.example"
        );
        let (status, choices) = call(&client, &owner, "GET", &ask, None);
        assert_eq!(200, status, "{name}: {choices}");
        assert_eq!("Agreements", choices["choices"][0]["name"], "{name}");
        // Not hers to do while self-restore is off.
        assert_eq!(403, call(&client, &olena, "GET", &ask, None).0, "{name}");
        let body = serde_json::json!({"point": point, "app": "notes", "what": "list", "user": "olena@ministry.example", "id": "l1"});
        assert_eq!(
            403,
            call(
                &client,
                &olena,
                "POST",
                "/backup/restore",
                Some(body.clone())
            )
            .0,
            "{name}"
        );
        let (status, done) = call(
            &client,
            &owner,
            "POST",
            "/backup/restore",
            Some(body.clone()),
        );
        assert_eq!(200, status, "{name}: {done}");
        assert!(
            done["name"].as_str().unwrap().starts_with("Agreements ("),
            "{name}: {done}"
        );
        assert_eq!("/notes?list=restored", done["path"], "{name}");
        // With self-restore on, her own (and nobody else's).
        call(
            &client,
            &owner,
            "PATCH",
            "/admin/backup",
            Some(serde_json::json!({"selfRestore": true})),
        );
        block_on(store.execute("DELETE FROM notes_lists", &[])).unwrap();
        assert_eq!(
            200,
            call(&client, &olena, "POST", "/backup/restore", Some(body)).0,
            "{name}"
        );
        let others = serde_json::json!({"point": point, "app": "notes", "what": "list", "user": "owner@ministry.example", "id": "l1"});
        assert_eq!(
            403,
            call(&client, &olena, "POST", "/backup/restore", Some(others)).0,
            "{name}"
        );
        // Something that was not there then.
        let missing = serde_json::json!({"point": point, "app": "notes", "what": "list", "user": "olena@ministry.example", "id": "l9"});
        assert_eq!(
            404,
            call(&client, &owner, "POST", "/backup/restore", Some(missing)).0,
            "{name}"
        );
    }
}

#[test]
fn names_and_times_agree() {
    for t in [
        0i64,
        1_795_197_600_000,
        1_609_459_199_000,
        4_102_444_800_000,
    ] {
        assert_eq!(Some(t), points::time_of(&points::name_at(t)), "{t}");
    }
    assert_eq!(None, points::time_of("not-a-point"));
}

#[test]
fn networks_group_ipv6_by_64() {
    use ortus_core::auth::limits::network;
    assert_eq!("2001:db8:1:2::/64", network("2001:db8:1:2:aaaa::1"));
    assert_eq!(network("2001:db8:1:2::5"), network("2001:db8:1:2:ffff::9"));
    assert_eq!("203.0.113.9", network("203.0.113.9"));
    assert_eq!("203.0.113.9", network("::ffff:203.0.113.9"));
}

/// One taker or pruner at a time; a changed list of files is refused; a
/// point still being taken is not pruned away.
#[test]
fn taking_and_pruning_take_turns() {
    let fresh = common::sqlite().unwrap();
    let store = (fresh.open)();
    let instance = testing_store::instance(Arc::clone(&store), vec![]);
    signed_in(&instance, "olena@ministry.example", Role::Owner);
    block_on(instance.files.put("drive/a.txt", b"shelter list")).unwrap();
    assert!(block_on(points::claim(&instance)).unwrap());
    assert!(!block_on(points::claim(&instance)).unwrap(), "held");
    block_on(points::release(&instance));
    assert!(block_on(points::claim(&instance)).unwrap());
    block_on(points::release(&instance));

    let dir = common::temp_dir();
    let dest = Files::local(&dir.0.join("backups")).unwrap();
    let now = ortus_core::program::now_ms();
    let name = block_on(points::take(&instance, &dest, now, true))
        .unwrap()
        .name;
    // A point being taken right now: files written, no manifest yet.
    let young = points::name_at(now + 1000);
    block_on(dest.put(&format!("points/{young}/files.jsonl"), b"")).unwrap();
    block_on(points::prune(&instance, &dest, now + 2000)).unwrap();
    assert!(
        block_on(dest.exists(&format!("points/{young}/files.jsonl"))).unwrap(),
        "kept while young"
    );

    // The list of files changed in the store: refused when read.
    let list = format!("points/{name}/files.jsonl");
    let text = block_on(dest.get(&list)).unwrap().unwrap();
    let snapshot = block_on(points::Snapshot::open(
        Files::local(&dir.0.join("backups")).unwrap(),
        &name,
    ))
    .unwrap();
    block_on(dest.put(&list, &[text.as_slice(), b"\n"].concat())).unwrap();
    assert!(block_on(snapshot.file("drive/a.txt")).is_err());

    // A new backup store: the index starts again, so every file is read.
    let other = Files::local(&dir.0.join("elsewhere")).unwrap();
    block_on(points::take(&instance, &other, now + 60_000, false)).unwrap();
    assert_eq!(1, block_on(other.list("blobs/")).unwrap().len());
}
