//! Ortus as an OpenID Connect provider: other services of the
//! organisation sign people in with their Ortus account.
//!
//! The authorization code flow with PKCE (S256, always), for clients an
//! administrator registers. ID tokens are JWTs signed with ES256 by a key
//! the installation makes the first time it needs one (kept in the
//! database, so every program of the installation signs with it). The
//! issuer is the installation's public address:
//!
//! - `/.well-known/openid-configuration` discovery
//! - `/oauth2/authorize` sends a person who is not signed in to the
//!   sign-in page and back
//! - `/oauth2/token`, `/oauth2/userinfo`, `/oauth2/jwks`

pub mod routes;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::storage::{AsyncStore, Migration, Schema, StoreError, StoreResult};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "openid connect provider",
    sql: "CREATE TABLE core_oidc_clients (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        secret_hash TEXT,
        redirect_uris TEXT NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE core_oidc_keys (
        kid TEXT PRIMARY KEY,
        pkcs8 {{blob}} NOT NULL,
        created_at BIGINT NOT NULL
    );
    CREATE TABLE core_oidc_codes (
        code_hash TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        user_id TEXT NOT NULL,
        redirect_uri TEXT NOT NULL,
        challenge TEXT NOT NULL,
        nonce TEXT,
        scope TEXT NOT NULL,
        methods TEXT NOT NULL,
        auth_time BIGINT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE TABLE core_oidc_tokens (
        token_hash TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        user_id TEXT NOT NULL,
        scope TEXT NOT NULL,
        expires_at BIGINT NOT NULL
    )",
}];

pub const SCHEMA: Schema = Schema {
    app: "core.oidc",
    migrations: MIGRATIONS,
};

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The signing key of the installation: made once, then reused.
pub struct SigningKey {
    pub kid: String,
    pair: EcdsaKeyPair,
}

impl SigningKey {
    /// The current key, made if there is none yet. Two programs that make
    /// one at the same time keep the first.
    pub async fn current(
        store: &dyn AsyncStore,
        keys: &crate::secrets::Keyring,
        now_ms: i64,
    ) -> StoreResult<Self> {
        let load = |rows: Vec<crate::storage::Row>| -> StoreResult<Option<Self>> {
            let Some(row) = rows.first() else {
                return Ok(None);
            };
            let pkcs8 = keys
                .open(row.blob(1).unwrap_or_default())
                .map_err(StoreError::Database)?;
            let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8)
                .map_err(|_| StoreError::Database("the OpenID signing key is damaged".into()))?;
            Ok(Some(Self {
                kid: row.text(0).unwrap_or_default().to_string(),
                pair,
            }))
        };
        const NEWEST: &str =
            "SELECT kid, pkcs8 FROM core_oidc_keys ORDER BY created_at, kid LIMIT 1";
        if let Some(key) = load(store.query(NEWEST, &[]).await?)? {
            return Ok(key);
        }
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| StoreError::Database("could not make a signing key".into()))?;
        let kid = crate::auth::random_hex(8);
        let mut tx = store.begin().await?;
        let existing = tx.query(NEWEST, &[]).await?;
        if existing.is_empty() {
            tx.execute(
                "INSERT INTO core_oidc_keys (kid, pkcs8, created_at) VALUES ($1, $2, $3)",
                &[
                    kid.as_str().into(),
                    keys.seal(pkcs8.as_ref()).into_bytes().into(),
                    now_ms.into(),
                ],
            )
            .await?;
        }
        tx.commit().await?;
        load(store.query(NEWEST, &[]).await?)?
            .ok_or_else(|| StoreError::Database("no signing key".into()))
    }

    /// The public key as a JWK.
    pub fn jwk(&self) -> Value {
        let point = self.pair.public_key().as_ref();
        json!({"kty": "EC", "crv": "P-256", "x": b64(&point[1..33]), "y": b64(&point[33..65]), "use": "sig", "alg": "ES256", "kid": self.kid})
    }

    /// `claims` as a compact JWS (ES256).
    pub fn sign(&self, claims: &Value) -> Result<String, String> {
        let header = json!({"alg": "ES256", "typ": "JWT", "kid": self.kid});
        let input = format!(
            "{}.{}",
            b64(header.to_string().as_bytes()),
            b64(claims.to_string().as_bytes())
        );
        let signature = self
            .pair
            .sign(&SystemRandom::new(), input.as_bytes())
            .map_err(|_| "signing failed".to_string())?;
        Ok(format!("{input}.{}", b64(signature.as_ref())))
    }
}

/// The S256 PKCE challenge of a verifier.
pub fn challenge_of(verifier: &str) -> String {
    b64(&Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_matches_rfc_7636() {
        // RFC 7636 appendix B.
        assert_eq!(
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
        );
    }
}
