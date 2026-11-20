//! Routes for apps outside the browser (`Access::Header`): signed in by
//! the Authorization header only — a session token or an app password —
//! never by a cookie, and without `X-Requested-With`. Both engines.

mod common;

use std::sync::Arc;

use base64::Engine;
use ortus_core::api::{answer, ok, ApiError, Method, Route};
use ortus_core::app::{App, AppInfo, Group};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store, Instance};
use ortus_core::storage::{AsyncStore, BoxFuture};
use rust_web_server::prelude::*;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

struct Sync;

fn whoami(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let result = async {
            let user = ortus_core::auth::by_header(&request, &ortus, "10.0.0.9")
                .await
                .ok_or_else(ApiError::sign_in)?;
            Ok::<_, ApiError>(ok(&json!({ "email": user.email })))
        }
        .await;
        answer(&request, result)
    })
}

fn any() -> Value {
    json!({"type": "object"})
}

impl App for Sync {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "sync",
            group: Group::Create,
            path: "/sync",
            order: 1,
        }
    }
    fn routes(&self) -> Vec<Route> {
        vec![Route {
            method: Method::Post,
            path: "/sync/whoami",
            id: "syncWhoami",
            tag: "sync",
            summary: "Who the app is signed in as.",
            access: ortus_core::auth::Access::Header,
            response: any,
            handler: whoami,
        }]
    }
}

fn header_access(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, vec![Arc::new(Sync)]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let token = signed_in(&instance, "olena@ministry.example", Role::Member);
    let made: Value = serde_json::from_str(
        client
            .post("/api/v1/me/app-passwords")
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .body_text(r#"{"name":"Calendar app"}"#)
            .send()
            .body_text(),
    )
    .unwrap();
    let password = made["password"].as_str().unwrap();
    let call = |authorization: Option<String>, cookie: Option<String>| {
        let mut request = client.post("/api/v1/sync/whoami").body_text("{}");
        if let Some(a) = authorization {
            request = request.header("Authorization", &a);
        }
        if let Some(c) = cookie {
            request = request.header("Cookie", &c);
        }
        let response = request.send();
        (response.status(), response.body_text().to_string())
    };
    let (status, body) = call(Some(format!("Bearer {token}")), None);
    assert!(
        status == 200 && body.contains("olena@ministry.example"),
        "{status} {body}"
    );
    let basic = base64::engine::general_purpose::STANDARD
        .encode(format!("olena@ministry.example:{password}"));
    let (status, body) = call(Some(format!("Basic {basic}")), None);
    assert!(
        status == 200 && body.contains("olena@ministry.example"),
        "{status} {body}"
    );
    let wrong = base64::engine::general_purpose::STANDARD
        .encode("olena@ministry.example:aaaa-bbbb-cccc-dddd");
    assert_eq!(401, call(Some(format!("Basic {wrong}")), None).0);
    // A signed-in browser's cookie counts for nothing here.
    for name in ["__Host-ortus", "ortus"] {
        let (status, _) = call(None, Some(format!("{name}={token}")));
        assert_eq!(401, status, "cookie {name}");
    }
    assert_eq!(401, call(Some("Bearer nonsense".into()), None).0);
}

#[test]
fn header_access_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    header_access((fresh.open)());
}

#[test]
fn header_access_on_postgres() {
    if let Some(fresh) = common::postgres() {
        header_access((fresh.open)());
    }
}
