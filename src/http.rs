//! The HTTP client Ortus uses to call other servers (S3, identity
//! providers). It trusts the public certificate authorities built into the
//! program (the webpki list), not the system's store: the container image
//! has none, and the program then works the same everywhere.

use std::sync::{Arc, OnceLock};

/// The shared client (connections are pooled across calls).
pub fn client() -> Result<reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
            reqwest::Client::builder()
                .tls_backend_preconfigured(tls)
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
}
