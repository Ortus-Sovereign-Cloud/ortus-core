//! Profile pictures through the API, on both engines: a person sets and
//! removes their own; colleagues see it by account or address; only real
//! images are taken; an administrator turns uploads off and removes one.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store};
use ortus_core::storage::AsyncStore;
use rust_web_server::test_client::TestClient;

/// A request as a signed-in person makes it.
fn with<'a, A: rust_web_server::application::Application>(
    token: &str,
    r: rust_web_server::test_client::TestRequest<'a, A>,
) -> rust_web_server::test_client::TestRequest<'a, A> {
    r.header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus")
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR this stands for an image";

fn photos(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
    let marek = signed_in(&instance, "marek@ministry.example", Role::Member);
    let taras = signed_in(&instance, "taras@ministry.example", Role::Admin);

    let olena_id = ortus_core::runtime::block_on(ortus_core::auth::accounts::by_email(
        instance.store.as_ref(),
        "olena@ministry.example",
    ))
    .unwrap()
    .unwrap()
    .id;

    // None yet: initials show.
    assert_eq!(
        204,
        with(
            &marek,
            client.get(&format!("/api/v1/people/{olena_id}/photo"))
        )
        .send()
        .status()
    );
    let mine = with(&olena, client.get("/api/v1/me/photo")).send();
    assert!(
        mine.body_text().contains("\"source\":null")
            && mine.body_text().contains("\"uploads_allowed\":true"),
        "{}",
        mine.body_text()
    );

    // Olena sets hers; Marek sees it, by account and by address.
    let set = with(&olena, client.put("/api/v1/me/photo"))
        .body_bytes(PNG.to_vec())
        .send();
    assert_eq!(200, set.status(), "{}", set.body_text());
    let seen = with(
        &marek,
        client.get(&format!("/api/v1/people/{olena_id}/photo")),
    )
    .send();
    assert_eq!(200, seen.status());
    assert_eq!(PNG, seen.body_bytes());
    assert_eq!(Some("image/png"), seen.header("Content-Type"));
    assert_eq!(Some("nosniff"), seen.header("X-Content-Type-Options"));
    let by_address = with(
        &marek,
        client.get("/api/v1/people/photo?email=Olena%40Ministry.example"),
    )
    .send();
    assert_eq!(200, by_address.status());
    // Not for anyone who is not signed in.
    assert_eq!(
        401,
        client
            .get(&format!("/api/v1/people/{olena_id}/photo"))
            .send()
            .status()
    );

    // Only PNG, JPEG or WebP, and small.
    let svg = with(&olena, client.put("/api/v1/me/photo"))
        .body_bytes(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec(),
        )
        .send();
    assert_eq!(400, svg.status());
    let mut big = PNG.to_vec();
    big.resize(600 * 1024, 0);
    assert_eq!(
        413,
        with(&olena, client.put("/api/v1/me/photo"))
            .body_bytes(big)
            .send()
            .status()
    );
    assert_eq!(
        PNG,
        with(
            &marek,
            client.get(&format!("/api/v1/people/{olena_id}/photo"))
        )
        .send()
        .body_bytes(),
        "the photo stays"
    );

    // She removes it.
    assert_eq!(
        200,
        with(&olena, client.delete("/api/v1/me/photo"))
            .send()
            .status()
    );
    assert_eq!(
        204,
        with(
            &marek,
            client.get(&format!("/api/v1/people/{olena_id}/photo"))
        )
        .send()
        .status()
    );

    // The administrator turns uploads off: refused; a member may not.
    assert_eq!(
        403,
        with(&marek, client.put("/api/v1/admin/photos"))
            .body_text("{\"allowed\":false}")
            .send()
            .status()
    );
    assert_eq!(
        200,
        with(&taras, client.put("/api/v1/admin/photos"))
            .body_text("{\"allowed\":false}")
            .send()
            .status()
    );
    let refused = with(&olena, client.put("/api/v1/me/photo"))
        .body_bytes(PNG.to_vec())
        .send();
    assert_eq!(403, refused.status());
    assert!(refused.body_text().contains("photos_off"));
    assert_eq!(
        200,
        with(&taras, client.put("/api/v1/admin/photos"))
            .body_text("{\"allowed\":true}")
            .send()
            .status()
    );

    // A photo that should not be there: the administrator removes it.
    assert_eq!(
        200,
        with(&olena, client.put("/api/v1/me/photo"))
            .body_bytes(PNG.to_vec())
            .send()
            .status()
    );
    assert_eq!(
        403,
        with(
            &marek,
            client.delete(&format!("/api/v1/admin/users/{olena_id}/photo"))
        )
        .send()
        .status()
    );
    assert_eq!(
        200,
        with(
            &taras,
            client.delete(&format!("/api/v1/admin/users/{olena_id}/photo"))
        )
        .send()
        .status()
    );
    assert_eq!(
        204,
        with(
            &marek,
            client.get(&format!("/api/v1/people/{olena_id}/photo"))
        )
        .send()
        .status()
    );
}

#[test]
fn photos_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    photos((fresh.open)());
}

#[test]
fn photos_on_postgres() {
    if let Some(fresh) = common::postgres() {
        photos((fresh.open)());
    }
}
