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
//! not verify the links).

use std::sync::Arc;

use rust_web_server::async_state::AsyncAppWithState;
use serde_json::{json, Value};

use crate::api::ok;
use crate::instance::Instance;

/// The paths these files are served at.
pub const PATHS: [&str; 2] = [
    "/.well-known/apple-app-site-association",
    "/.well-known/assetlinks.json",
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
                "components": [{ "/": "/welcome/accept", "comment": "Invitations open in the app." }],
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

/// Adds both files.
pub fn register(app: AsyncAppWithState<Arc<Instance>>) -> AsyncAppWithState<Arc<Instance>> {
    app.get(PATHS[0], |_, _, _, _: Arc<Arc<Instance>>| async {
        ok(&apple())
    })
    .get(PATHS[1], |_, _, _, _: Arc<Arc<Instance>>| async {
        ok(&android())
    })
}
