//! The web client as the program serves it.

use rust_web_server::test_client::TestClient;

fn client() -> TestClient<impl rust_web_server::application::Application> {
    TestClient::new(ortus_core::app())
}

#[test]
fn the_page_is_served_and_always_revalidated() {
    let response = client().get("/").send();
    assert_eq!(200, response.status());
    assert!(response
        .header("Content-Type")
        .unwrap()
        .starts_with("text/html"));
    assert_eq!(Some("no-cache"), response.header("Cache-Control"));
    let page = response.body_text();
    assert!(page.contains("<ortus-shell>"), "{page}");
    assert!(
        page.contains("lang=\"en\"") && page.contains("dir=\"ltr\""),
        "the page starts in English, left to right"
    );
}

#[test]
fn the_page_allows_nothing_from_outside_or_inline() {
    let response = client().get("/").send();
    let policy = response
        .header("Content-Security-Policy")
        .expect("a content security policy");
    assert!(policy.contains("default-src 'self'"));
    assert!(policy.contains("frame-ancestors 'none'"));
    assert!(
        !policy.contains("unsafe-inline")
            && !policy.contains("unsafe-eval")
            && !policy.contains("http"),
        "{policy}"
    );
    assert_eq!(Some("nosniff"), response.header("X-Content-Type-Options"));
    let page = response.body_text();
    assert!(
        !page.contains("https://") && !page.contains("http://"),
        "the page must not load anything from another origin"
    );
    assert!(
        !page.contains("<style") && !page.contains(" style=") && !page.contains("<script>"),
        "no inline style or script"
    );
}

#[test]
fn every_file_the_page_names_is_served_and_cached_for_a_year() {
    let client = client();
    let home = client.get("/").send();
    let page = home.body_text();
    let assets: Vec<&str> = page
        .split('"')
        .filter(|part| part.starts_with("/assets/"))
        .collect();
    assert!(
        assets.iter().any(|path| path.ends_with(".js"))
            && assets.iter().any(|path| path.ends_with(".css")),
        "{assets:?}"
    );
    for path in assets {
        let response = client.get(path).send();
        assert_eq!(200, response.status(), "{path}");
        assert_eq!(
            Some("public, max-age=31536000, immutable"),
            response.header("Cache-Control"),
            "{path}"
        );
        let expected = if path.ends_with(".js") {
            "text/javascript"
        } else {
            "text/css"
        };
        assert!(
            response
                .header("Content-Type")
                .unwrap()
                .starts_with(expected),
            "{path}"
        );
    }
    assert_eq!(200, client.get("/favicon.svg").send().status());
}

#[test]
fn files_that_do_not_exist_are_not_found() {
    let client = client();
    for path in [
        "/assets/nothing.js",
        "/assets/../index.html",
        "/assets/..%2F..%2FCargo.toml",
        "/assets/",
    ] {
        assert_eq!(404, client.get(path).send().status(), "{path}");
    }
}

#[test]
fn the_api_and_the_probe_still_answer_beside_the_client() {
    let client = client();
    for probe in ["/healthz", "/livez"] {
        assert_eq!(200, client.get(probe).send().status(), "{probe}");
    }
    // Ready only once a server runs; the test client has none, but the
    // probe must reach rws rather than be refused as an unknown path.
    assert!(client.get("/readyz").send().body_text().contains("ready"));
    let apps = client.get("/api/v1/apps").send();
    assert_eq!(200, apps.status());
    assert_eq!("{\"apps\":[]}", apps.body_text());
}

#[test]
fn files_beside_the_program_are_not_served() {
    // The test runs in the repository root, which holds these files; a
    // real installation has its database and configuration there.
    let client = client();
    for path in [
        "/Cargo.toml",
        "/src/lib.rs",
        "/deny.toml",
        "/web/package.json",
        "/LICENSE-MIT",
        "/README.md",
        "/src/",
        "/api/../Cargo.toml",
        "/api/v1/../../Cargo.toml",
        "/assets/../../Cargo.toml",
        "/assets/%2e%2e/%2e%2e/Cargo.toml",
        "/assets/..%2f..%2fCargo.toml",
        "/api/%2E%2E/Cargo.toml",
        "/api//../Cargo.toml",
        "/healthz/../Cargo.toml",
        "/Cargo.toml?x=/api/",
    ] {
        let response = client.get(path).send();
        assert_eq!(404, response.status(), "{path} must not be served");
        assert!(
            !response.body_text().contains("[package]"),
            "{path} leaked a file"
        );
    }
}

#[test]
fn only_ortus_paths_are_let_through() {
    use ortus_core::web::is_served;
    for path in [
        "/",
        "/?lang=pl",
        "/healthz",
        "/livez",
        "/readyz",
        "/favicon.svg",
        "/assets/index-abc.js",
        "/api/v1/health",
        "/api/v1/apps?x=1",
        "/api/v2/anything",
    ] {
        assert!(is_served(path), "{path}");
    }
    for path in [
        "",
        "/index.html",
        "/data/ortus.db",
        "/ortus.toml",
        "/assets",
        "/apix",
        "/healthz2",
        "/api/..",
        "/assets/a\\b",
        "/assets/a%5Cb",
        "/api/%00",
    ] {
        assert!(!is_served(path), "{path}");
    }
}
