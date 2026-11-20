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
