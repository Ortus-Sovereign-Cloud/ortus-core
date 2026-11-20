//! Secrets at rest: what the database keeps that would let someone act as
//! the installation (its OIDC signing key, outside providers' client
//! secrets, the directory password, authenticator-app secrets) is sealed
//! with AES-256-GCM under a master key kept outside the database.
//!
//! The key comes from `ORTUS_SECRETS_KEY` (base64 keys, comma separated,
//! the first one current), or else from `<data_dir>/secrets.key` (one key
//! a line, the first current), made with mode 0600 on the first start.
//! Older keys stay to open what they sealed (and older backups).
//!
//! A sealed value reads `enc:v1:<key id>:<base64 nonce+ciphertext+tag>`.
//! Values written before sealing existed are plain; they are sealed at
//! start, and read as they are meanwhile.

use std::path::{Path, PathBuf};

use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::storage::{AsyncStore, StoreResult};

const PREFIX: &str = "enc:v1:";

/// Where the key lives, for the Sovereignty screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Custody {
    Environment,
    File(PathBuf),
    /// Tests only.
    Fixed,
}

pub struct Keyring {
    /// (id, key); the first is current.
    keys: Vec<(String, LessSafeKey)>,
    pub custody: Custody,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("ids", &self.ids())
            .field("custody", &self.custody)
            .finish()
    }
}

fn key_id(raw: &[u8]) -> String {
    hex::encode(&Sha256::digest(raw)[..4])
}

fn parse_key(text: &str) -> Result<(String, LessSafeKey), String> {
    let raw = STANDARD
        .decode(text.trim())
        .map_err(|_| "a secrets key is base64".to_string())?;
    if raw.len() != 32 {
        return Err("a secrets key is 32 bytes (44 base64 characters)".into());
    }
    let key =
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &raw).map_err(|_| "a bad secrets key")?);
    Ok((key_id(&raw), key))
}

/// A new random key, base64.
pub fn new_key() -> String {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).expect("the operating system's random numbers");
    STANDARD.encode(raw)
}

impl Keyring {
    pub fn from_keys(lines: &[String], custody: Custody) -> Result<Self, String> {
        let keys = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| parse_key(l))
            .collect::<Result<Vec<_>, _>>()?;
        if keys.is_empty() {
            return Err("no secrets key".into());
        }
        Ok(Self { keys, custody })
    }

    /// The installation's keyring: the environment, or the key file in the
    /// data directory (made on the first start).
    pub fn load(data_dir: &Path, env: Option<String>) -> Result<Self, String> {
        if let Some(value) = env.filter(|v| !v.trim().is_empty()) {
            let lines: Vec<String> = value.split(',').map(str::to_string).collect();
            return Self::from_keys(&lines, Custody::Environment)
                .map_err(|e| format!("ORTUS_SECRETS_KEY: {e}"));
        }
        let path = data_dir.join("secrets.key");
        if !path.exists() {
            write_key_file(&path, &[new_key()])?;
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        Self::from_keys(&lines, Custody::File(path.clone()))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A fixed key, for tests.
    pub fn for_tests() -> Self {
        Self::from_keys(&[STANDARD.encode([7u8; 32])], Custody::Fixed).expect("a test key")
    }

    pub fn ids(&self) -> Vec<String> {
        self.keys.iter().map(|(id, _)| id.clone()).collect()
    }

    pub fn current_id(&self) -> &str {
        &self.keys[0].0
    }

    pub fn seal(&self, plain: &[u8]) -> String {
        let (id, key) = &self.keys[0];
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("the operating system's random numbers");
        let mut data = plain.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(id.as_bytes()),
            &mut data,
        )
        .expect("AES-GCM seals");
        let mut out = nonce.to_vec();
        out.extend(data);
        format!("{PREFIX}{id}:{}", STANDARD.encode(out))
    }

    pub fn seal_text(&self, plain: &str) -> String {
        self.seal(plain.as_bytes())
    }

    /// Opens a sealed value; a plain (older) value comes back as it is.
    pub fn open(&self, value: &[u8]) -> Result<Vec<u8>, String> {
        let Some(rest) = value.strip_prefix(PREFIX.as_bytes()) else {
            return Ok(value.to_vec());
        };
        let rest = std::str::from_utf8(rest).map_err(|_| "a sealed value is text")?;
        let (id, body) = rest.split_once(':').ok_or("a sealed value names its key")?;
        let key = self
            .keys
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, key)| key)
            .ok_or_else(|| {
                format!("no key {id} to open a secret (was the secrets key changed?)")
            })?;
        let mut data = STANDARD
            .decode(body)
            .map_err(|_| "a sealed value is base64")?;
        if data.len() < NONCE_LEN + 16 {
            return Err("a sealed value is too short".into());
        }
        let nonce: [u8; NONCE_LEN] = data[..NONCE_LEN].try_into().expect("the nonce length");
        let opened = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(id.as_bytes()),
                &mut data[NONCE_LEN..],
            )
            .map_err(|_| "a sealed value does not open (wrong key or changed)")?
            .to_vec();
        Ok(opened)
    }

    pub fn open_text(&self, value: &str) -> Result<String, String> {
        String::from_utf8(self.open(value.as_bytes())?).map_err(|_| "a secret is not text".into())
    }

    /// Whether a value is sealed with the current key.
    pub fn is_current(&self, value: &[u8]) -> bool {
        value.starts_with(format!("{PREFIX}{}:", self.current_id()).as_bytes())
    }
}

/// Writes a key file readable by its owner only.
pub fn write_key_file(path: &Path, keys: &[String]) -> Result<(), String> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let temp = path.with_extension("key.new");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("{}: {e}", temp.display()))?;
    file.write_all(format!("{}\n", keys.join("\n")).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Where sealed values live: (table, key column, secret column, whether
/// the secret is a JSON document whose `secret` field is sealed).
const PLACES: &[(&str, &str, &str, bool)] = &[
    ("core_oidc_keys", "kid", "pkcs8", false),
    ("core_external_providers", "id", "client_secret", false),
    ("core_directory", "id", "password", false),
    ("core_credentials", "id", "data", true),
];

/// Seals every plain secret, and re-seals what an older key sealed, with
/// the current key. Answers how many values changed.
pub async fn seal_all(store: &dyn AsyncStore, keys: &Keyring) -> Result<u64, String> {
    let mut changed = 0;
    let mut tx = store.begin().await.map_err(|e| e.to_string())?;
    for (table, id_column, column, json) in PLACES {
        let filter = if *json {
            " WHERE kind IN ('totp', 'totp-pending')"
        } else {
            ""
        };
        let rows = tx
            .query(
                &format!("SELECT {id_column}, {column} FROM {table}{filter}"),
                &[],
            )
            .await
            .map_err(|e| e.to_string())?;
        for row in rows {
            let id = row
                .text(0)
                .map(str::to_string)
                .or_else(|| row.int(0).map(|n| n.to_string()))
                .unwrap_or_default();
            let new = if *json {
                let Some(text) = row.text(1) else { continue };
                let mut doc: serde_json::Value =
                    serde_json::from_str(text).map_err(|e| e.to_string())?;
                let Some(secret) = doc["secret"].as_str().map(str::to_string) else {
                    continue;
                };
                if keys.is_current(secret.as_bytes()) {
                    continue;
                }
                doc["secret"] = keys.seal(&keys.open(secret.as_bytes())?).into();
                crate::storage::Value::from(doc.to_string())
            } else {
                let bytes: Vec<u8> = match (row.blob(1), row.text(1)) {
                    (Some(b), _) => b.to_vec(),
                    (None, Some(t)) => t.as_bytes().to_vec(),
                    _ => continue,
                };
                if keys.is_current(&bytes) {
                    continue;
                }
                let sealed = keys.seal(&keys.open(&bytes)?);
                if *table == "core_oidc_keys" {
                    crate::storage::Value::from(sealed.into_bytes())
                } else {
                    crate::storage::Value::from(sealed)
                }
            };
            let key: crate::storage::Value = if *table == "core_directory" {
                id.parse::<i64>().unwrap_or(1).into()
            } else {
                id.as_str().into()
            };
            tx.execute(
                &format!("UPDATE {table} SET {column} = $1 WHERE {id_column} = $2"),
                &[new, key],
            )
            .await
            .map_err(|e| e.to_string())?;
            changed += 1;
        }
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(changed)
}

/// Checks that `keys` opens what this database sealed, and remembers a
/// check value the first time. A database sealed under another key is
/// refused, rather than started half-blind.
pub async fn check(store: &dyn AsyncStore, keys: &Keyring) -> Result<(), String> {
    let stored = crate::auth::policy::setting(store, "secrets_check")
        .await
        .map_err(|e| e.to_string())?;
    match stored {
        Some(value) => match keys.open_text(&value) {
            Ok(text) if text == "ortus" => Ok(()),
            _ => Err("the secrets key does not open this database's secrets: start with the key it was made with (ORTUS_SECRETS_KEY or secrets.key)".into()),
        },
        None => crate::auth::policy::set_setting(store, "secrets_check", &keys.seal_text("ortus")).await.map_err(|e| e.to_string()),
    }
}

/// Facts for the Sovereignty screen.
pub async fn facts(store: &dyn AsyncStore, keys: &Keyring) -> StoreResult<serde_json::Value> {
    let rotated = crate::auth::policy::setting(store, "keys_rotated_at")
        .await?
        .and_then(|t| t.parse::<i64>().ok());
    Ok(serde_json::json!({
        "custody": match &keys.custody { Custody::Environment => "environment", Custody::File(_) => "file", Custody::Fixed => "fixed" },
        "algorithm": "AES-256-GCM",
        "key_id": keys.current_id(),
        "keys": keys.ids().len(),
        "rotated_at": rotated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_values_open_and_name_their_key() {
        let keys = Keyring::for_tests();
        let sealed = keys.seal_text("client secret");
        assert!(sealed.starts_with("enc:v1:") && !sealed.contains("client secret"));
        assert_eq!("client secret", keys.open_text(&sealed).unwrap());
        assert_ne!(
            sealed,
            keys.seal_text("client secret"),
            "a fresh nonce each time"
        );
        assert_eq!(
            "plain from before",
            keys.open_text("plain from before").unwrap()
        );
        // Tampered, or under another key: refused.
        let mut tampered = sealed.clone().into_bytes();
        let last = tampered.len() - 3;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(keys.open(&tampered).is_err());
        let other = Keyring::from_keys(&[new_key()], Custody::Fixed).unwrap();
        assert!(other.open(sealed.as_bytes()).is_err());
        // An older key in the ring still opens.
        let rotated =
            Keyring::from_keys(&[new_key(), STANDARD.encode([7u8; 32])], Custody::Fixed).unwrap();
        assert_eq!("client secret", rotated.open_text(&sealed).unwrap());
        assert!(!rotated.is_current(sealed.as_bytes()));
    }
}
