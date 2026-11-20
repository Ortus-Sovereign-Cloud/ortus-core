//! An app's own web client is served from the app, under
//! /assets/apps/<id>/, with the same headers as every Ortus file.

use std::borrow::Cow;
use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};
use ortus_core::instance::testing;
use rust_web_server::test_client::TestClient;

struct Notes;

impl App for Notes {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "notes",
            group: Group::Create,
            path: "/notes",
            order: 1,
        }
    }
    fn web_file(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        match path {
            "app.js" => Some(Cow::Borrowed(b"import '/assets/kit.js';")),
            "chunk-abc123.js" => Some(Cow::Borrowed(b"export const x = 1;")),
            _ => None,
        }
    }
}

#[test]
fn an_app_serves_its_own_client() {
    let instance = testing::instance(vec![Arc::new(Notes)]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let entry = client.get("/assets/apps/notes/app.js").send();
    assert_eq!(200, entry.status());
    assert_eq!("import '/assets/kit.js';", entry.body_text());
    assert_eq!(
        Some("no-cache"),
        entry.header("cache-control"),
        "the entry is asked for again after a release"
    );
    assert!(entry
        .header("content-type")
        .unwrap_or_default()
        .contains("javascript"));
    assert!(entry.header("content-security-policy").is_some());
    let chunk = client.get("/assets/apps/notes/chunk-abc123.js").send();
    assert!(chunk
        .header("cache-control")
        .unwrap_or_default()
        .contains("immutable"));
    for missing in [
        "/assets/apps/notes/nothing.js",
        "/assets/apps/drive/app.js",
        "/assets/apps/notes/../../index.html",
    ] {
        assert_ne!(200, client.get(missing).send().status(), "{missing}");
    }
    // The kit is there for apps, and asked for again after a release.
    let kit = client.get("/assets/kit.js").send();
    assert_eq!(200, kit.status());
    assert_eq!(Some("no-cache"), kit.header("cache-control"));
}
