//! Labels, partners and the one policy function, on both engines. The
//! slice's done-when: a label blocks a forbidden share, and the attempt is
//! in the audit log (through a small app that shares the way Drive will).

mod common;

use std::sync::Arc;

use ortus_core::api::{answer, body, ok, ApiError, Method, Route};
use ortus_core::app::{App, AppInfo, Group};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, testing_store, Instance};
use ortus_core::policy::{self, Ask, Audience, Channel};
use ortus_core::storage::{AsyncStore, BoxFuture};
use rust_web_server::prelude::*;
use rust_web_server::test_client::{TestClient, TestResponse};
use serde::Deserialize;
use serde_json::{json, Value};

/// A cabinet that shares its folders the way Drive will.
struct Cabinet;

#[derive(Deserialize)]
struct Share {
    label: Option<String>,
    to: String,
    reason: Option<String>,
}

fn share(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    ortus: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        let result = async {
            let me = ortus_core::auth::current(&request, &ortus)
                .await
                .ok_or_else(ApiError::sign_in)?;
            let input: Share = body(&request)?;
            let audience = if input.to == "anyone" {
                Audience::Anyone
            } else {
                Audience::Outside(input.to.clone())
            };
            let ask = Ask {
                actor: &me.user.id,
                action: policy::Action::Share,
                label: input.label.as_deref(),
                audience: &audience,
                channel: Channel::Files,
                object: "cabinet:folder:budget",
                reason: input.reason.as_deref(),
            };
            let decision = policy::decide(&ortus, &ask).await?;
            if decision.allowed() {
                Ok(ok(&json!({ "shared": true, "decision": decision })))
            } else {
                Err(ApiError {
                    status: 403,
                    code: "policy",
                    message: serde_json::to_string(&decision).unwrap(),
                })
            }
        }
        .await;
        answer(&request, result)
    })
}

fn any() -> Value {
    json!({"type": "object"})
}

impl App for Cabinet {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "cabinet",
            group: Group::Create,
            path: "/cabinet",
            order: 1,
        }
    }
    fn routes(&self) -> Vec<Route> {
        vec![Route {
            method: Method::Post,
            path: "/cabinet/share",
            id: "shareFolder",
            tag: "cabinet",
            summary: "Share a folder.",
            access: ortus_core::auth::Access::Signed,
            response: any,
            handler: share,
        }]
    }
}

fn json_of(response: &TestResponse) -> Value {
    serde_json::from_str(response.body_text())
        .unwrap_or_else(|e| panic!("{e}: {}", response.body_text()))
}

fn labels(store: Arc<dyn AsyncStore>) {
    let instance = testing_store::instance(store, vec![Arc::new(Cabinet)]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let owner = signed_in(&instance, "taras@ministry.example", Role::Owner);
    let admin = signed_in(&instance, "marek@ministry.example", Role::Admin);
    let sam = signed_in(&instance, "sam@ministry.example", Role::Member);
    let send = |method: &str, path: &str, token: &str, body: Value| {
        let request = match method {
            "GET" => client.get(path),
            "PUT" => client.put(path),
            "DELETE" => client.delete(path),
            _ => client.post(path),
        };
        request
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .body_text(&body.to_string())
            .send()
    };
    let share = |label: Option<&str>, to: &str, reason: Option<&str>| {
        send(
            "POST",
            "/api/v1/cabinet/share",
            &sam,
            json!({"label": label, "to": to, "reason": reason}),
        )
    };

    // The four labels come with the installation, by rank.
    let policy = json_of(&send("GET", "/api/v1/admin/policy", &admin, Value::Null));
    let names: Vec<&str> = policy["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        vec!["PUBLIC", "INTERNAL", "OFFICIAL-SENSITIVE", "SECRET"],
        names
    );
    assert_eq!(
        403,
        send("GET", "/api/v1/admin/policy", &sam, Value::Null).status()
    );

    // The done-when: SECRET does not go outside, and the attempt is audited.
    let refused = share(Some("secret"), "resident@mail.example", None);
    assert_eq!(403, refused.status());
    assert!(
        refused.body_text().contains("\\\"rule\\\":\\\"outside\\\""),
        "{}",
        refused.body_text()
    );
    let audit = json_of(&send(
        "GET",
        "/api/v1/admin/audit?action=policy.refused",
        &owner,
        Value::Null,
    ));
    let event = &audit["events"][0];
    assert_eq!(
        ("policy.refused", "refused"),
        (
            event["action"].as_str().unwrap(),
            event["outcome"].as_str().unwrap()
        ),
        "{audit}"
    );
    let detail = event["detail"].to_string();
    assert!(
        detail.contains("cabinet:folder:budget")
            && detail.contains("mail.example")
            && detail.contains("secret"),
        "{detail}"
    );

    // INTERNAL goes outside with a reason, which is audited too.
    assert!(share(None, "resident@mail.example", None)
        .body_text()
        .contains("needs_reason"));
    assert_eq!(
        200,
        share(
            None,
            "resident@mail.example",
            Some("the resident asked for it")
        )
        .status()
    );
    let overridden = json_of(&send(
        "GET",
        "/api/v1/admin/audit?action=policy.overridden",
        &owner,
        Value::Null,
    ));
    assert!(
        overridden["events"][0]["detail"]
            .to_string()
            .contains("the resident asked for it"),
        "{overridden}"
    );

    // Partners: only for the channels they were allowed.
    assert_eq!(
        403,
        send(
            "POST",
            "/api/v1/admin/policy/partners",
            &admin,
            json!({"domain": "health.example.gov", "name": "Health", "channels": ["mail"]})
        )
        .status(),
        "owners only"
    );
    assert_eq!(
        200,
        send(
            "POST",
            "/api/v1/admin/policy/partners",
            &owner,
            json!({"domain": "health.example.gov", "name": "Health", "channels": ["mail", "chat"]})
        )
        .status()
    );
    assert!(
        share(
            Some("official-sensitive"),
            "doctor@clinic.health.example.gov",
            None
        )
        .body_text()
        .contains("\\\"rule\\\":\\\"outside\\\""),
        "files are not a channel this partner has"
    );
    send(
        "POST",
        "/api/v1/admin/policy/partners",
        &owner,
        json!({"domain": "health.example.gov", "name": "Health", "channels": ["files"]}),
    );
    assert!(
        share(
            Some("official-sensitive"),
            "doctor@clinic.health.example.gov",
            None
        )
        .body_text()
        .contains("needs_reason"),
        "partners are warned for OFFICIAL-SENSITIVE"
    );
    assert_eq!(
        403,
        share(Some("secret"), "doctor@health.example.gov", Some("urgent")).status(),
        "a reason does not open a deny"
    );

    // Links for anyone, an unknown label (the strictest), PUBLIC.
    assert_eq!(403, share(None, "anyone", Some("please")).status());
    assert_eq!(
        403,
        share(
            Some("no-such-label"),
            "resident@mail.example",
            Some("please")
        )
        .status()
    );
    assert_eq!(200, share(Some("public"), "anyone", None).status());

    // The owner changes a label; the check endpoint follows it.
    let mut rules = policy["labels"][3]["rules"].clone();
    rules["partners"] = json!("warn");
    assert_eq!(
        403,
        send(
            "PUT",
            "/api/v1/admin/policy/labels/secret",
            &admin,
            json!({"name": "SECRET", "rank": 3, "rules": rules})
        )
        .status()
    );
    assert_eq!(
        200,
        send(
            "PUT",
            "/api/v1/admin/policy/labels/secret",
            &owner,
            json!({"name": "SECRET", "rank": 3, "rules": rules})
        )
        .status()
    );
    let checked = json_of(&send(
        "POST",
        "/api/v1/policy/check",
        &sam,
        json!({"action": "share", "label": "secret", "to": "health.example.gov", "channel": "files"}),
    ));
    assert_eq!("needs_reason", checked["decision"]);
    assert_eq!(
        400,
        send(
            "DELETE",
            "/api/v1/admin/policy/labels/internal",
            &owner,
            Value::Null
        )
        .status(),
        "the default label stays"
    );
    assert_eq!(
        200,
        send(
            "DELETE",
            "/api/v1/admin/policy/partners/health.example.gov",
            &owner,
            Value::Null
        )
        .status()
    );
}

#[test]
fn labels_on_sqlite() {
    let fresh = common::sqlite().unwrap();
    labels((fresh.open)());
}

#[test]
fn labels_on_postgres() {
    if let Some(fresh) = common::postgres() {
        labels((fresh.open)());
    }
}
