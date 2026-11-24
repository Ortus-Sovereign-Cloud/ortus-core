//! What a request costs in the database, as the `Server-Timing` header
//! says it (ORTUS_SERVER_TIMING): a signed-in request looks its session up
//! once, however often the route and its handler ask.

use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing;
use rust_web_server::test_client::TestClient;

fn statements(timing: &str) -> u32 {
    let (_, rest) = timing.split_once("desc=\"").expect(timing);
    rest.split(' ').next().unwrap().parse().unwrap()
}

#[test]
fn reading_who_one_is_takes_two_statements() {
    std::env::set_var("ORTUS_SERVER_TIMING", "1");
    let instance = testing::instance(Vec::new());
    let token = ortus_core::instance::testing_auth::signed_in(
        &instance,
        "iryna@ministry.example",
        Role::Member,
    );
    let client = TestClient::new(ortus_core::app(instance));
    for _ in 0..3 {
        let me = client
            .get("/api/v1/me")
            .header("Cookie", &format!("ortus={token}"))
            .send();
        assert_eq!(200, me.status(), "{}", me.body_text());
        let timing = me.header("Server-Timing").expect("a Server-Timing header");
        // The session and its account; the guard and the handler share them.
        assert!(statements(timing) <= 2, "{timing}");
    }
}
