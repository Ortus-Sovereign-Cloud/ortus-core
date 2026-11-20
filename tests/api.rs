//! The public API's skeleton: versioned paths, one error format, a
//! request id on everything, and a reference that matches the code.

use ortus_core::api;
use rust_web_server::test_client::TestClient;
use serde_json::Value;

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

#[test]
fn health_answers_under_the_versioned_prefix() {
    let response = TestClient::new(ortus_core::app(ortus_core::instance::testing::instance(
        Vec::new(),
    )))
    .get("/api/v1/health")
    .send();
    assert_eq!(200, response.status());
    assert_eq!(Some("application/json"), response.header("Content-Type"));
    let body = json(response.body_text());
    assert_eq!("ok", body["status"]);
    assert_eq!(ortus_core::VERSION, body["version"]);
}

#[test]
fn every_response_has_a_request_id_and_keeps_the_callers() {
    let client = TestClient::new(ortus_core::app(ortus_core::instance::testing::instance(
        Vec::new(),
    )));
    let generated = client.get("/api/v1/health").send();
    let first = generated
        .header("X-Request-Id")
        .expect("a generated id")
        .to_string();
    assert!(first.len() >= 16, "{first}");
    assert_ne!(
        Some(first.as_str()),
        client.get("/api/v1/health").send().header("X-Request-Id"),
        "ids are unique per request"
    );

    let echoed = client
        .get("/api/v1/health")
        .header("X-Request-Id", "from-the-caller-123")
        .send();
    assert_eq!(Some("from-the-caller-123"), echoed.header("X-Request-Id"));
}

#[test]
fn unknown_api_paths_get_the_error_format_for_every_method() {
    let client = TestClient::new(ortus_core::app(ortus_core::instance::testing::instance(
        Vec::new(),
    )));
    let responses = [
        client
            .get("/api/v1/nothing/here")
            .header("X-Request-Id", "req-42")
            .send(),
        client
            .post("/api/v1/nothing")
            .header("X-Request-Id", "req-42")
            .send(),
        client
            .put("/api/v1/nothing")
            .header("X-Request-Id", "req-42")
            .send(),
        client
            .patch("/api/v1/nothing")
            .header("X-Request-Id", "req-42")
            .send(),
        client
            .delete("/api/v1/health/deeper")
            .header("X-Request-Id", "req-42")
            .send(),
    ];
    for response in responses {
        assert_eq!(404, response.status());
        let body = json(response.body_text());
        assert_eq!("not_found", body["error"]["code"]);
        assert_eq!("req-42", body["error"]["request_id"]);
        assert!(body["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()));
        assert_eq!(Some("req-42"), response.header("X-Request-Id"));
    }
}

#[test]
fn an_error_without_a_caller_id_reports_the_generated_one() {
    let response = TestClient::new(ortus_core::app(ortus_core::instance::testing::instance(
        Vec::new(),
    )))
    .get("/api/v1/nothing")
    .send();
    let body = json(response.body_text());
    let id = response.header("X-Request-Id").expect("a generated id");
    assert_eq!(id, body["error"]["request_id"].as_str().unwrap());
    assert!(!id.is_empty());
}

#[test]
fn the_reference_lists_every_route_and_no_others() {
    let routes = api::routes();
    let reference = api::reference(&routes);
    assert_eq!("3.1.0", reference["openapi"]);
    let paths = reference["paths"].as_object().unwrap();
    let mut documented = 0;
    for route in &routes {
        let path = format!("{}{}", api::PREFIX, route.path);
        assert!(path.starts_with("/api/v1/"), "{path}");
        let operation = &paths[&path][match route.method {
            api::Method::Get => "get",
            api::Method::Post => "post",
            api::Method::Put => "put",
            api::Method::Patch => "patch",
            api::Method::Delete => "delete",
        }];
        assert_eq!(route.id, operation["operationId"], "{path}");
        assert!(
            operation["responses"]["default"]["content"]["application/json"]["schema"]["$ref"]
                .as_str()
                .unwrap()
                .ends_with("/Error")
        );
        documented += 1;
    }
    let operations: usize = paths
        .values()
        .map(|path| path.as_object().unwrap().len())
        .sum();
    assert_eq!(documented, operations);
    let mut ids: Vec<&str> = routes.iter().map(|route| route.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(routes.len(), ids.len(), "operation ids are unique");
}

/// The reference in the repository is the one the code generates. After
/// changing a route, run `ORTUS_UPDATE_REFERENCE=1 cargo test --test api`
/// and commit the file.
#[test]
fn the_checked_in_reference_is_up_to_date() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("api/openapi.json");
    let generated = api::reference_text(&api::routes());
    if std::env::var("ORTUS_UPDATE_REFERENCE").is_ok() {
        std::fs::write(&file, &generated).unwrap();
    }
    let checked_in = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(checked_in == generated, "api/openapi.json is stale: run ORTUS_UPDATE_REFERENCE=1 cargo test --test api and commit it");
}
