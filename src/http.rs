//! The HTTP client Ortus uses to call other servers (S3, identity
//! providers). It trusts the public certificate authorities built into the
//! program (the webpki list), not the system's store: the container image
//! has none, and the program then works the same everywhere.

use std::sync::{Arc, OnceLock};

/// A TLS configuration trusting the built-in authorities, and `extra_ca`
/// (PEM) when an internal service has its own.
pub fn tls_config(extra_ca: Option<&str>) -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = extra_ca.filter(|p| !p.trim().is_empty()) {
        for cert in rustls::pki_types::pem::PemObject::pem_slice_iter(pem.as_bytes()) {
            let cert: rustls::pki_types::CertificateDer<'static> =
                cert.map_err(|e| format!("the CA certificate: {e}"))?;
            roots
                .add(cert)
                .map_err(|e| format!("the CA certificate: {e}"))?;
        }
    }
    Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth())
}

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

/// Where systems keep their certificate authorities.
const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

/// Libraries that read the system's certificate authorities (the DNS
/// resolver of mail, the directory's LDAP) find none in the container
/// image, which has no system: they get the program's own (the same
/// public authorities, CDLA-Permissive-2.0), written once to a file
/// that `SSL_CERT_FILE` names. A system with its own list, or a set
/// `SSL_CERT_FILE`, is left alone.
pub fn ensure_ca_bundle() {
    if std::env::var_os("SSL_CERT_FILE").is_some()
        || SYSTEM_BUNDLES
            .iter()
            .any(|p| std::path::Path::new(p).exists())
    {
        return;
    }
    let dir = std::env::var_os("ORTUS_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let file = dir.join(".ca-certificates.pem");
    let mut pem = String::new();
    for cert in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(cert.as_ref());
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        for line in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap_or_default());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
    }
    let same = std::fs::read_to_string(&file).is_ok_and(|kept| kept == pem);
    if same || std::fs::write(&file, &pem).is_ok() {
        // Set before any other thread reads the environment (the start of
        // the program).
        std::env::set_var("SSL_CERT_FILE", &file);
    }
}
