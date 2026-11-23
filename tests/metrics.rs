//! Review A9: the metrics endpoint answers only with ORTUS_METRICS_TOKEN
//! (404 otherwise, as if it did not exist), in Prometheus text format.

mod common;

use std::sync::Arc;

use ortus_core::instance::testing_store;
use rust_web_server::test_client::TestClient;

#[test]
fn metrics_need_their_token() {
    let fresh = common::sqlite().unwrap();
    let instance = testing_store::instance((fresh.open)(), Vec::new());
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let get = |token: Option<&str>| {
        let r = client.get("/api/v1/metrics");
        let r = match token {
            Some(t) => r.header("Authorization", &format!("Bearer {t}")),
            None => r,
        };
        let out = r.send();
        (out.status(), out.body_text().to_string())
    };
    // Off when no token is set.
    unsafe { std::env::remove_var("ORTUS_METRICS_TOKEN") };
    assert_eq!(404, get(Some("anything-at-all-here")).0);
    unsafe { std::env::set_var("ORTUS_METRICS_TOKEN", "a-long-enough-metrics-token") };
    assert_eq!(404, get(None).0);
    assert_eq!(404, get(Some("a-long-enough-metrics-tokex")).0);
    let (status, text) = get(Some("a-long-enough-metrics-token"));
    assert_eq!(200, status, "{text}");
    for name in [
        "ortus_jobs_oldest_due_seconds ",
        "ortus_sessions ",
        "ortus_push_devices ",
        "ortus_restore_point_age_seconds -1",
    ] {
        assert!(text.contains(name), "{name} in {text}");
    }
}
