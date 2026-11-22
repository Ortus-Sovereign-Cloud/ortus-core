//! Removing a person, on both engines: only an owner, who types the
//! address again; never oneself, the last owner or someone under a hold;
//! everything of theirs goes (a photo and a phone code too, which refer
//! to the account), and their address is free again.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::{self, Role};
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::runtime::block_on;
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;

fn with<'a, A: rust_web_server::application::Application>(
    token: &str,
    r: rust_web_server::test_client::TestRequest<'a, A>,
) -> rust_web_server::test_client::TestRequest<'a, A> {
    r.header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus")
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR this stands for an image";

fn remove_person(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "bohdan@gmail.example", Role::Owner);
    let admin = signed_in(&instance, "marek@ministry.example", Role::Admin);
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let s = instance.store.as_ref();
    let id_of = |email: &str| {
        block_on(accounts::by_email(s, email))
            .unwrap()
            .map(|u| u.id)
    };
    let olena_id = id_of("olena@ministry.example").unwrap();
    let owner_id = id_of("bohdan@gmail.example").unwrap();

    // Olena has a photo and a phone code: rows that refer to her account.
    assert_eq!(
        200,
        with(&olena, client.put("/api/v1/me/photo"))
            .body_bytes(PNG.to_vec())
            .send()
            .status()
    );
    assert_eq!(
        200,
        with(&olena, client.post("/api/v1/me/phone-link"))
            .body_text("{}")
            .send()
            .status()
    );

    let remove = |token: &str, id: &str, confirm: &str| {
        with(token, client.delete(&format!("/api/v1/admin/users/{id}")))
            .body_text(&format!("{{\"confirm\":\"{confirm}\"}}"))
            .send()
    };

    // An administrator who is not an owner may not; the owner must type it.
    assert_eq!(
        403,
        remove(&admin, &olena_id, "olena@ministry.example").status()
    );
    assert_eq!(400, remove(&owner, &olena_id, "olena@").status());
    // Not oneself.
    assert_eq!(
        409,
        remove(&owner, &owner_id, "bohdan@gmail.example").status()
    );

    // Under a hold: kept.
    block_on(s.execute(
        "INSERT INTO core_holds (id, case_ref, note, created_by, created_at) VALUES ('h1', 'CASE-1', '', $1, 1)",
        &[owner_id.as_str().into()],
    ))
    .unwrap();
    block_on(s.execute(
        "INSERT INTO core_hold_targets (hold_id, scope, target) VALUES ('h1', 'person', $1)",
        &[olena_id.as_str().into()],
    ))
    .unwrap();
    let held = remove(&owner, &olena_id, "olena@ministry.example");
    assert_eq!(409, held.status());
    assert!(held.body_text().contains("on_hold"));
    block_on(s.execute("UPDATE core_holds SET released_at = 2 WHERE id = 'h1'", &[])).unwrap();

    // Removed: gone, signed out, and the address is free.
    let removed = remove(&owner, &olena_id, " Olena@Ministry.example ");
    assert_eq!(200, removed.status(), "{}", removed.body_text());
    assert_eq!(None, id_of("olena@ministry.example"));
    assert_eq!(401, with(&olena, client.get("/api/v1/me")).send().status());
    for table in ["core_photos", "core_phone_links", "core_sessions"] {
        let left = block_on(s.query(
            &format!("SELECT COUNT(*) FROM {table} WHERE user_id = $1"),
            &[olena_id.as_str().into()],
        ))
        .unwrap()[0]
            .int(0);
        assert_eq!(Some(0), left, "{table}");
    }
    let audit = block_on(s.query(
        "SELECT COUNT(*) FROM core_audit WHERE action = 'people.removed'",
        &[],
    ))
    .unwrap()[0]
        .int(0);
    assert_eq!(Some(1), audit);
    // Somebody new can have the address.
    signed_in(&instance, "olena@ministry.example", Role::Member);
    assert!(id_of("olena@ministry.example").is_some());
}

#[test]
fn remove_person_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    remove_person((fresh.open)());
}

#[test]
fn remove_person_on_postgres() {
    if let Some(fresh) = common::postgres() {
        remove_person((fresh.open)());
    }
}
