//! Ortus core: the part of Ortus every installation has.
//!
//! The core builds the rws application that the `ortus` programs serve.
//! Apps (mail, drive, chat and the rest) will register with it; for now it
//! answers the health check of the public API.

pub mod storage;

use rust_web_server::prelude::*;
use rust_web_server::state::AppWithState;

/// Version of the core, as reported by the health check.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Path prefix of the public API. Every route of the API lives under it.
pub const API_PREFIX: &str = "/api/v1";

fn health(_: &Request, _: &PathParams, _: &ConnectionInfo, _: &()) -> Response {
    let body = format!("{{\"status\":\"ok\",\"version\":\"{VERSION}\"}}");
    Response::json(STATUS_CODE_REASON_PHRASE.n200_ok, body.into_bytes())
}

/// The core application. Requests not routed here fall through to the rws
/// built-ins, which include the plain `/healthz` probe.
pub fn app() -> AppWithState<()> {
    App::with_state(()).get(&format!("{API_PREFIX}/health"), health)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_web_server::test_client::TestClient;

    #[test]
    fn health_check_answers_ok_with_the_version() {
        let response = TestClient::new(app()).get("/api/v1/health").send();
        assert_eq!(200, response.status());
        assert_eq!(
            format!("{{\"status\":\"ok\",\"version\":\"{VERSION}\"}}"),
            response.body_text()
        );
    }

    #[test]
    fn builtin_probe_still_answers() {
        assert_eq!(200, TestClient::new(app()).get("/healthz").send().status());
    }
}
