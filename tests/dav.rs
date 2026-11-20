//! The WebDAV mount: `/dav/<app>/…` reaches the app that has a handler,
//! with the rest of the path; the well-known names it claims redirect;
//! nothing else under `/dav/` or `/.well-known/` is served.

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};
use ortus_core::instance::Instance;
use ortus_core::storage::BoxFuture;
use rust_web_server::prelude::*;
use rust_web_server::test_client::TestClient;

struct Cards;

fn handler(
    request: Request,
    _: ConnectionInfo,
    _: Arc<Instance>,
    rest: String,
) -> BoxFuture<'static, Response> {
    Box::pin(async move {
        Response::text(
            STATUS_CODE_REASON_PHRASE.n200_ok,
            &format!("{} {rest}", request.method),
        )
    })
}

impl App for Cards {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "cards",
            group: Group::Organise,
            path: "/cards",
            order: 1,
        }
    }
    fn dav(&self) -> Option<ortus_core::dav::DavHandler> {
        Some(handler)
    }
    fn well_known(&self) -> Vec<(&'static str, &'static str)> {
        vec![("carddav", "/dav/cards/")]
    }
}

#[test]
fn the_mount_reaches_its_app() {
    let instance = ortus_core::instance::testing::instance(vec![Arc::new(Cards)]);
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let r = client.get("/dav/cards/olena/contacts/").send();
    assert_eq!((200, "GET /olena/contacts/"), (r.status(), r.body_text()));
    assert_eq!(
        "PUT /x.vcf",
        client
            .put("/dav/cards/x.vcf")
            .body_text("x")
            .send()
            .body_text()
    );
    assert_eq!(
        404,
        client.get("/dav/other/x").send().status(),
        "no such app"
    );
    assert_eq!(
        404,
        client.get("/dav/cards/../../secrets.key").send().status(),
        "no climbing out"
    );
    let moved = client.get("/.well-known/carddav").send();
    assert_eq!(301, moved.status());
    assert_eq!(
        Some("/dav/cards/".to_string()),
        moved.header("location").map(str::to_string)
    );
    assert_eq!(
        404,
        client.get("/.well-known/caldav").send().status(),
        "unclaimed"
    );
    // An app that is switched off answers nothing.
    ortus_core::runtime::block_on(instance.switches.set("cards", false, 1)).unwrap();
    assert_eq!(404, client.get("/dav/cards/x").send().status());
    // Pure path checks.
    assert!(ortus_core::dav::served(
        "/dav/cards/a",
        &["cards".into()],
        &[]
    ));
    assert!(!ortus_core::dav::served(
        "/dav/mail/a",
        &["cards".into()],
        &[]
    ));
    assert!(ortus_core::dav::served(
        "/.well-known/carddav",
        &[],
        &["carddav".into()]
    ));
}
