//! Secrets at rest, on both engines: values written before sealing are
//! sealed at start, read back the same, and a wrong key is refused.

mod common;

use std::sync::Arc;

use ortus_core::runtime::block_on;
use ortus_core::secrets::{self, Custody, Keyring};
use ortus_core::storage::AsyncStore;

fn sealing(store: Arc<dyn AsyncStore>) {
    block_on(ortus_core::storage::install(
        store.as_ref(),
        &ortus_core::schemas(),
        0,
    ))
    .unwrap();
    let s = store.as_ref();
    let run = |sql: &str| block_on(s.execute(sql, &[])).unwrap();
    // Plain secrets, as an installation from before sealing kept them.
    run("INSERT INTO core_users (id, email, name, role, state, created_at, chooser_seen) VALUES ('u1', 'taras@ministry.example', 'Taras', 'owner', 'active', 1, 0)");
    run("INSERT INTO core_credentials (id, user_id, kind, name, data, created_at) VALUES ('c1', 'u1', 'totp', 'App', '{\"secret\":\"JBSWY3DPEHPK3PXP\",\"last_step\":5}', 1)");
    run("INSERT INTO core_external_providers (id, name, issuer, client_id, client_secret, scopes, link_by_email, create_accounts, created_at) VALUES ('eid', 'eID', 'https://idp.example', 'c', 'client-secret', 'openid', 0, 0, 1)");
    run("INSERT INTO core_directory (id, url, bind_dn, password, base_dn, user_filter, group_filter, mail_attribute, name_attribute) VALUES (1, 'ldaps://d', 'cn=a', 'directory-password', 'dc=x', 'f', 'g', 'mail', 'cn')");

    let keys = Keyring::for_tests();
    block_on(secrets::check(s, &keys)).unwrap();
    assert_eq!(3, block_on(secrets::seal_all(s, &keys)).unwrap());
    assert_eq!(
        0,
        block_on(secrets::seal_all(s, &keys)).unwrap(),
        "sealing twice changes nothing"
    );

    let one = |sql: &str| {
        block_on(s.query(sql, &[])).unwrap()[0]
            .text(0)
            .unwrap()
            .to_string()
    };
    let data: serde_json::Value =
        serde_json::from_str(&one("SELECT data FROM core_credentials WHERE id = 'c1'")).unwrap();
    assert_eq!(5, data["last_step"], "the rest of the document stays");
    let totp = data["secret"].as_str().unwrap();
    assert!(totp.starts_with("enc:v1:"));
    assert_eq!("JBSWY3DPEHPK3PXP", keys.open_text(totp).unwrap());
    let client = one("SELECT client_secret FROM core_external_providers WHERE id = 'eid'");
    assert_eq!("client-secret", keys.open_text(&client).unwrap());
    let password = one("SELECT password FROM core_directory WHERE id = 1");
    assert!(!password.contains("directory-password"));
    assert_eq!("directory-password", keys.open_text(&password).unwrap());

    // Another key does not open this database.
    let other = Keyring::from_keys(&[secrets::new_key()], Custody::Fixed).unwrap();
    assert!(block_on(secrets::check(s, &other)).is_err());

    // Rotated by the environment ("new,old"): everything, the check value
    // too, is re-sealed, so the old key can then be dropped.
    // An app's sealed values (a whole column, and fields of a JSON
    // document) are sealed and re-sealed with the rest.
    run("CREATE TABLE app_things (id TEXT PRIMARY KEY, secret TEXT, doc TEXT)");
    run("INSERT INTO app_things (id, secret, doc) VALUES ('t1', 'app-password', '{\"rsa\":\"key-one\",\"other\":\"left alone\"}')");
    run("INSERT INTO app_things (id, secret, doc) VALUES ('t2', NULL, NULL)");
    const PLACES: &[secrets::Place] = &[
        secrets::Place {
            table: "app_things",
            id: "id",
            column: "secret",
            fields: &[],
        },
        secrets::Place {
            table: "app_things",
            id: "id",
            column: "doc",
            fields: &["rsa"],
        },
    ];
    assert_eq!(
        2,
        block_on(secrets::seal_all_with(s, &keys, PLACES)).unwrap()
    );
    assert_eq!(
        0,
        block_on(secrets::seal_all_with(s, &keys, PLACES)).unwrap()
    );
    let doc: serde_json::Value =
        serde_json::from_str(&one("SELECT doc FROM app_things WHERE id = 't1'")).unwrap();
    assert_eq!("left alone", doc["other"]);
    assert_eq!(
        "key-one",
        keys.open_text(doc["rsa"].as_str().unwrap()).unwrap()
    );

    let new = secrets::new_key();
    let both = Keyring::from_keys(&[new.clone(), base64_of_test_key()], Custody::Fixed).unwrap();
    block_on(secrets::seal_all_with(s, &both, PLACES)).unwrap();
    let only_new = Keyring::from_keys(&[new], Custody::Fixed).unwrap();
    block_on(secrets::check(s, &only_new)).unwrap();
    assert_eq!(
        "app-password",
        only_new
            .open_text(&one("SELECT secret FROM app_things WHERE id = 't1'"))
            .unwrap(),
        "re-sealed under the new key"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&one("SELECT doc FROM app_things WHERE id = 't1'")).unwrap();
    assert_eq!(
        "key-one",
        only_new.open_text(doc["rsa"].as_str().unwrap()).unwrap()
    );

    // A process still holding only the old key cannot read the new seals:
    // its code check refuses, rather than treating the secret as empty.
    let step = ortus_core::program::now_ms() / 30_000;
    let empty_key_code = ortus_core::auth::credentials::totp(&[], step);
    assert!(!block_on(ortus_core::auth::credentials::check_totp(
        s,
        &keys,
        "u1",
        &empty_key_code,
        step * 30_000
    ))
    .unwrap());
    let real = ortus_core::auth::credentials::totp(
        &ortus_core::auth::credentials::unbase32("JBSWY3DPEHPK3PXP").unwrap(),
        step,
    );
    assert!(
        !block_on(ortus_core::auth::credentials::check_totp(
            s,
            &keys,
            "u1",
            &real,
            step * 30_000
        ))
        .unwrap(),
        "unreadable under the old key"
    );
    assert!(block_on(ortus_core::auth::credentials::check_totp(
        s,
        &only_new,
        "u1",
        &real,
        step * 30_000
    ))
    .unwrap());
}

fn base64_of_test_key() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode([7u8; 32])
}

#[test]
fn sealing_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    sealing((fresh.open)());
}

#[test]
fn sealing_on_postgres() {
    if let Some(fresh) = common::postgres() {
        sealing((fresh.open)());
    }
}
