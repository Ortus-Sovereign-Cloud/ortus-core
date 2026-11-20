//! Ways to prove who you are, other than passkeys: a password (Argon2id),
//! a one-time code from an authenticator app (TOTP, RFC 6238), and the
//! recovery kit (ten single-use codes for when everything else is lost).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use hmac::{Hmac, Mac};
use serde_json::{json, Value as Json};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::storage::{AsyncStore, StoreResult};

/// The shortest password Ortus accepts.
pub const MIN_PASSWORD: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Password,
    /// A one-time-code secret waiting for its first code.
    TotpPending,
    Totp,
    Recovery,
    Passkey,
    /// A generated password for one mail, calendar or contacts app.
    AppPassword,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Password => "password",
            Kind::TotpPending => "totp-pending",
            Kind::Totp => "totp",
            Kind::Recovery => "recovery",
            Kind::Passkey => "passkey",
            Kind::AppPassword => "app-password",
        }
    }
}

/// A stored credential.
#[derive(Clone, Debug)]
pub struct Credential {
    pub id: String,
    pub user_id: String,
    pub kind: String,
    pub name: String,
    pub data: Json,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

pub async fn of_kind(
    store: &dyn AsyncStore,
    user_id: &str,
    kind: &Kind,
) -> StoreResult<Vec<Credential>> {
    Ok(store
        .query(
            "SELECT id, user_id, kind, name, data, created_at, last_used_at FROM core_credentials WHERE user_id = $1 AND kind = $2 ORDER BY created_at",
            &[user_id.into(), kind.name().into()],
        )
        .await?
        .iter()
        .map(|row| Credential {
            id: row.text(0).unwrap_or_default().into(),
            user_id: row.text(1).unwrap_or_default().into(),
            kind: row.text(2).unwrap_or_default().into(),
            name: row.text(3).unwrap_or_default().into(),
            data: serde_json::from_str(row.text(4).unwrap_or("null")).unwrap_or(Json::Null),
            created_at: row.int(5).unwrap_or_default(),
            last_used_at: row.int(6),
        })
        .collect())
}

/// Every credential of an account, for listing (secrets left out).
pub async fn all(store: &dyn AsyncStore, user_id: &str) -> StoreResult<Vec<Credential>> {
    let mut out = Vec::new();
    for kind in [Kind::Password, Kind::Totp, Kind::Recovery, Kind::Passkey] {
        for mut credential in of_kind(store, user_id, &kind).await? {
            credential.data = Json::Null;
            out.push(credential);
        }
    }
    Ok(out)
}

/// Adds a credential; with `replace`, removes the account's others of
/// the same kind first.
pub async fn put(
    store: &dyn AsyncStore,
    user_id: &str,
    kind: &Kind,
    name: &str,
    data: &Json,
    replace: bool,
    now_ms: i64,
) -> StoreResult<String> {
    let id = format!("c_{}", super::random_hex(12));
    let mut tx = store.begin().await?;
    if replace {
        tx.execute(
            "DELETE FROM core_credentials WHERE user_id = $1 AND kind = $2",
            &[user_id.into(), kind.name().into()],
        )
        .await?;
    }
    tx.execute(
        "INSERT INTO core_credentials (id, user_id, kind, name, data, created_at, last_used_at) VALUES ($1, $2, $3, $4, $5, $6, NULL)",
        &[id.as_str().into(), user_id.into(), kind.name().into(), name.into(), data.to_string().into(), now_ms.into()],
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn update(store: &dyn AsyncStore, id: &str, data: &Json, now_ms: i64) -> StoreResult<()> {
    store
        .execute(
            "UPDATE core_credentials SET data = $1, last_used_at = $2 WHERE id = $3",
            &[data.to_string().into(), now_ms.into(), id.into()],
        )
        .await?;
    Ok(())
}

/// Removes one credential of an account.
pub async fn remove(store: &dyn AsyncStore, user_id: &str, id: &str) -> StoreResult<bool> {
    Ok(store
        .execute(
            "DELETE FROM core_credentials WHERE id = $1 AND user_id = $2",
            &[id.into(), user_id.into()],
        )
        .await?
        == 1)
}

// --- Passwords ---

/// Why a password is not accepted.
pub fn password_problem(password: &str) -> Option<&'static str> {
    if password.chars().count() < MIN_PASSWORD {
        Some("too_short")
    } else if password.len() > 1024 {
        Some("too_long")
    } else {
        None
    }
}

pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("the operating system's random numbers");
    let salt = SaltString::encode_b64(&salt).expect("a 16-byte salt");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 hashes any password")
        .to_string()
}

pub fn password_matches(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// Spends the same time as checking a real password, so a wrong address
/// cannot be told from a wrong password by the delay.
pub fn waste_a_password_check(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("ortus-dummy-password"));
    let _ = password_matches(password, hash);
}

pub async fn set_password(
    store: &dyn AsyncStore,
    user_id: &str,
    password: &str,
    now_ms: i64,
) -> StoreResult<()> {
    put(
        store,
        user_id,
        &Kind::Password,
        "Password",
        &json!({ "hash": hash_password(password) }),
        true,
        now_ms,
    )
    .await?;
    Ok(())
}

pub async fn check_password(
    store: &dyn AsyncStore,
    user_id: &str,
    password: &str,
    now_ms: i64,
) -> StoreResult<bool> {
    let stored = of_kind(store, user_id, &Kind::Password).await?;
    let Some(credential) = stored.first() else {
        waste_a_password_check(password);
        return Ok(false);
    };
    let ok = password_matches(
        password,
        credential.data["hash"].as_str().unwrap_or_default(),
    );
    if ok {
        update(store, &credential.id, &credential.data, now_ms).await?;
    }
    Ok(ok)
}

// --- One-time codes (TOTP) ---

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            out.push(BASE32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for c in text.chars().filter(|c| *c != '=' && !c.is_whitespace()) {
        let value = BASE32
            .iter()
            .position(|b| *b as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// The six-digit code for `secret` in the 30-second step `step`.
pub fn totp(secret: &[u8], step: i64) -> String {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret).expect("HMAC takes any key");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let number = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:06}", number % 1_000_000)
}

/// The step a code typed at `now_ms` matches, allowing one step of clock
/// drift either way, and never a step at or before `last_step` (a code
/// works once).
/// An authenticator secret, if it opens and is one.
fn open_secret(keys: &crate::secrets::Keyring, data: &Json) -> Option<Vec<u8>> {
    let text = keys.open_text(data["secret"].as_str()?).ok()?;
    unbase32(&text).filter(|secret| secret.len() >= 10)
}

pub fn totp_step(secret: &[u8], code: &str, now_ms: i64, last_step: i64) -> Option<i64> {
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    // An empty secret (one that could not be read) never matches: anyone
    // could compute its codes.
    if code.len() != 6 || secret.len() < 10 {
        return None;
    }
    let now = now_ms.div_euclid(30_000);
    (now - 1..=now + 1).find(|step| *step > last_step && totp(secret, *step) == code)
}

/// Starts enrolling an authenticator app: a new secret, waiting for its
/// first code. Returns the secret (base32) and the `otpauth://` address
/// the app reads from a QR code.
pub async fn start_totp(
    store: &dyn AsyncStore,
    keys: &crate::secrets::Keyring,
    user_id: &str,
    email: &str,
    issuer: &str,
    now_ms: i64,
) -> StoreResult<(String, String)> {
    let mut secret = [0u8; 20];
    getrandom::fill(&mut secret).expect("the operating system's random numbers");
    let encoded = base32(&secret);
    put(
        store,
        user_id,
        &Kind::TotpPending,
        "Authenticator app",
        // Sealed at rest (crate::secrets).
        &json!({ "secret": keys.seal_text(&encoded) }),
        true,
        now_ms,
    )
    .await?;
    let label = format!("{issuer}:{email}");
    let uri = format!(
        "otpauth://totp/{}?secret={encoded}&issuer={}&algorithm=SHA1&digits=6&period=30",
        percent(&label),
        percent(issuer)
    );
    Ok((encoded, uri))
}

fn percent(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'@' | b':' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Finishes enrolling with the first code from the app.
pub async fn confirm_totp(
    store: &dyn AsyncStore,
    keys: &crate::secrets::Keyring,
    user_id: &str,
    code: &str,
    now_ms: i64,
) -> StoreResult<bool> {
    let pending = of_kind(store, user_id, &Kind::TotpPending).await?;
    let Some(credential) = pending.first() else {
        return Ok(false);
    };
    // A secret that does not open refuses; it never becomes an empty key.
    let Some(secret) = open_secret(keys, &credential.data) else {
        return Ok(false);
    };
    let Some(step) = totp_step(&secret, code, now_ms, i64::MIN) else {
        return Ok(false);
    };
    let data = json!({ "secret": credential.data["secret"], "last_step": step });
    put(
        store,
        user_id,
        &Kind::Totp,
        "Authenticator app",
        &data,
        true,
        now_ms,
    )
    .await?;
    remove(store, user_id, &credential.id).await?;
    Ok(true)
}

pub async fn has_totp(store: &dyn AsyncStore, user_id: &str) -> StoreResult<bool> {
    Ok(!of_kind(store, user_id, &Kind::Totp).await?.is_empty())
}

pub async fn check_totp(
    store: &dyn AsyncStore,
    keys: &crate::secrets::Keyring,
    user_id: &str,
    code: &str,
    now_ms: i64,
) -> StoreResult<bool> {
    for credential in of_kind(store, user_id, &Kind::Totp).await? {
        let Some(secret) = open_secret(keys, &credential.data) else {
            continue;
        };
        let last = credential.data["last_step"].as_i64().unwrap_or(i64::MIN);
        if let Some(step) = totp_step(&secret, code, now_ms, last) {
            let data = json!({ "secret": credential.data["secret"], "last_step": step });
            update(store, &credential.id, &data, now_ms).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

// --- Recovery kit ---

/// A recovery code as people read it: three groups of four letters and
/// digits, without the ones that look alike.
fn recovery_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("the operating system's random numbers");
    let letters: String = bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}-{}", &letters[0..4], &letters[4..8], &letters[8..12])
}

fn code_hash(code: &str) -> String {
    let normal: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    hex::encode(Sha256::digest(normal.as_bytes()))
}

/// Makes a new recovery kit (the old one stops working) and returns its
/// ten codes. They are shown once; only their hashes are kept.
pub async fn new_recovery_kit(
    store: &dyn AsyncStore,
    user_id: &str,
    now_ms: i64,
) -> StoreResult<Vec<String>> {
    let codes: Vec<String> = (0..10).map(|_| recovery_code()).collect();
    let hashes: Vec<String> = codes.iter().map(|c| code_hash(c)).collect();
    put(
        store,
        user_id,
        &Kind::Recovery,
        "Recovery kit",
        &json!({ "hashes": hashes }),
        true,
        now_ms,
    )
    .await?;
    Ok(codes)
}

/// Uses one recovery code. Returns how many are left, or `None` when the
/// code is not one of them.
pub async fn use_recovery_code(
    store: &dyn AsyncStore,
    user_id: &str,
    code: &str,
    now_ms: i64,
) -> StoreResult<Option<usize>> {
    let wanted = code_hash(code);
    for credential in of_kind(store, user_id, &Kind::Recovery).await? {
        let mut hashes: Vec<String> = credential.data["hashes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(position) = hashes.iter().position(|h| *h == wanted) {
            hashes.remove(position);
            update(store, &credential.id, &json!({ "hashes": hashes }), now_ms).await?;
            return Ok(Some(hashes.len()));
        }
    }
    Ok(None)
}

pub async fn recovery_codes_left(store: &dyn AsyncStore, user_id: &str) -> StoreResult<usize> {
    Ok(of_kind(store, user_id, &Kind::Recovery)
        .await?
        .first()
        .and_then(|c| c.data["hashes"].as_array().map(Vec::len))
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_the_rfc_6238_vectors() {
        // RFC 6238 appendix B, SHA-1, eight digits truncated to six.
        let secret = b"12345678901234567890";
        assert_eq!("287082", totp(secret, 59 / 30));
        assert_eq!("081804", totp(secret, 1_111_111_109 / 30));
        assert_eq!("050471", totp(secret, 1_111_111_111 / 30));
        assert_eq!("005924", totp(secret, 1_234_567_890 / 30));
        assert_eq!("279037", totp(secret, 2_000_000_000 / 30));
    }

    #[test]
    fn a_code_works_once_and_only_near_its_time() {
        let secret = b"12345678901234567890";
        let now = 1_111_111_109_000;
        let code = totp(secret, now / 30_000);
        let step = totp_step(secret, &code, now, i64::MIN).unwrap();
        assert_eq!(None, totp_step(secret, &code, now, step));
        assert!(totp_step(secret, &code, now + 30_000, i64::MIN).is_some());
        assert_eq!(None, totp_step(secret, &code, now + 90_000, i64::MIN));
        assert_eq!(None, totp_step(secret, "12345", now, i64::MIN));
    }

    #[test]
    fn base32_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foobar", &[0u8, 255, 17, 42, 99]] {
            assert_eq!(Some(bytes.to_vec()), unbase32(&base32(bytes)));
        }
        assert_eq!("MZXW6YTBOI", base32(b"foobar"));
    }

    #[test]
    fn passwords_hash_and_check() {
        let hash = hash_password("correct horse battery");
        assert!(password_matches("correct horse battery", &hash));
        assert!(!password_matches("correct horse batterY", &hash));
        assert_ne!(
            hash,
            hash_password("correct horse battery"),
            "every hash has its own salt"
        );
        assert_eq!(Some("too_short"), password_problem("short"));
        assert_eq!(None, password_problem("long enough password"));
    }

    #[test]
    fn recovery_codes_read_well() {
        let code = recovery_code();
        assert_eq!(14, code.len());
        assert_eq!(
            code_hash(&code),
            code_hash(&code.to_lowercase().replace('-', " "))
        );
    }
}
