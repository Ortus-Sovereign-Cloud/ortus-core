//! The files phones read to open this installation's invitation links in
//! the Ortus app instead of the browser: Apple's
//! `/.well-known/apple-app-site-association` and Android's
//! `/.well-known/assetlinks.json`. Only `/welcome/accept` is claimed.
//!
//! An app opens these links only for domains it names itself (the official
//! apps name ortus.cc); for any other installation the invitation page
//! offers the app by its `ortus://` link instead.
//!
//! Settings (environment): `ORTUS_IOS_APP_IDS` (team id and bundle id,
//! comma separated; default the official app, `XD7JHZ5DB3.cc.ortus.app`),
//! `ORTUS_ANDROID_APP` (package; default `cc.ortus.app`) and
//! `ORTUS_ANDROID_CERT_SHA256` (the signing certificates' SHA-256
//! fingerprints, comma separated; none by default, and then Android does
//! not verify the links), `ORTUS_INVITE_APP_LINK` (the shared app link an
//! invitation also carries, which the official apps open on any
//! installation; default `https://join.ortus.cc/i`, empty for none).
//!
//! The shared app link names the installation and the code after `#`, so
//! neither reaches the server behind it; its page (`/i`, on every
//! installation) shows where the invitation leads and goes there only
//! when asked.
//!
//! `/.well-known/ortus` names this installation, so the apps can find it
//! from a work email address: served here, and by an organisation's own
//! website when its mail domain differs.

use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;
use serde_json::{json, Value};

use crate::api::ok;
use crate::instance::Instance;

/// The paths these files are served at.
pub const PATHS: [&str; 3] = [
    "/.well-known/apple-app-site-association",
    "/.well-known/assetlinks.json",
    "/.well-known/ortus",
];

fn list(name: &str, default: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Apple's file: the invitation path, for the app ids.
pub fn apple() -> Value {
    let apps = list("ORTUS_IOS_APP_IDS", "XD7JHZ5DB3.cc.ortus.app");
    json!({
        "applinks": {
            "details": [{
                "appIDs": apps,
                "components": [
                    { "/": "/welcome/accept", "comment": "Invitations open in the app." },
                    { "/": "/i", "comment": "The shared app link of invitations." },
                ],
            }],
        },
    })
}

/// Android's file: the package and its signing certificates.
pub fn android() -> Value {
    let package = list("ORTUS_ANDROID_APP", "cc.ortus.app");
    let fingerprints = list("ORTUS_ANDROID_CERT_SHA256", "");
    Value::Array(
        package
            .into_iter()
            .map(|package| {
                json!({
                    "relation": ["delegate_permission/common.handle_all_urls"],
                    "target": {
                        "namespace": "android_app",
                        "package_name": package,
                        "sha256_cert_fingerprints": fingerprints,
                    },
                })
            })
            .collect(),
    )
}

/// The shared app link's base, if invitations carry one.
pub fn app_link_base() -> Option<String> {
    let base = std::env::var("ORTUS_INVITE_APP_LINK")
        .unwrap_or_else(|_| "https://join.ortus.cc/i".to_string());
    let base = base.trim().trim_end_matches('/').to_string();
    (!base.is_empty()).then_some(base)
}

/// The shared app link of an invitation: `<base>#s=<installation>&c=<code>`.
pub fn app_link(base: &str, public_url: &str, code: &str) -> String {
    let encoded: String = public_url
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("{base}#s={encoded}&c={code}")
}

/// Adds the files.
pub fn register(app: AsyncAppWithState<Arc<Instance>>) -> AsyncAppWithState<Arc<Instance>> {
    app.get(PATHS[0], |_, _, _, _: Arc<Arc<Instance>>| async {
        ok(&apple())
    })
    .get(PATHS[1], |_, _, _, _: Arc<Arc<Instance>>| async {
        ok(&android())
    })
    .get(PATHS[2], |_, _, _, ortus: Arc<Arc<Instance>>| async move {
        ok(&json!({ "server": ortus.config.public_url }))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shared_app_link_carries_the_installation_and_code_after_the_hash() {
        assert_eq!(
            "https://join.ortus.cc/i#s=https%3A%2F%2Fwork.ministry.gov%3A8443&c=abc_DEF-123",
            super::app_link(
                "https://join.ortus.cc/i",
                "https://work.ministry.gov:8443",
                "abc_DEF-123"
            )
        );
    }
}
