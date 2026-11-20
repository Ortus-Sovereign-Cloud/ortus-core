//! Passkeys: the WebAuthn checks of registration and sign-in (W3C Web
//! Authentication, level 2), with ES256, EdDSA and RS256 keys.
//!
//! Attestation is not checked: Ortus asks for none and trusts the key the
//! person registers while signed in. Everything that guards sign-in is
//! checked: the challenge (one use, five minutes), the origin, the relying
//! party, the user-present flag (and user-verified when the policy wants
//! it), the signature, and the signature counter going forward.

use aws_lc_rs::signature::{
    RsaPublicKeyComponents, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ED25519,
    RSA_PKCS1_2048_8192_SHA256,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::cbor::{self, Cbor};

/// Who asks for the passkey: this installation.
#[derive(Clone, Debug)]
pub struct RelyingParty {
    /// The host name, as `ortus.example.org`.
    pub id: String,
    /// The origin the browser reports, as `https://ortus.example.org`.
    pub origin: String,
}

impl RelyingParty {
    /// From the installation's public address.
    pub fn from_public_url(public_url: &str) -> Self {
        let origin = public_url.trim_end_matches('/').to_string();
        let host_port = origin
            .split("://")
            .nth(1)
            .unwrap_or_default()
            .split('/')
            .next()
            .unwrap_or_default();
        let id = host_port
            .rsplit_once(':')
            .map_or(host_port, |(host, port)| {
                if port.bytes().all(|b| b.is_ascii_digit()) {
                    host
                } else {
                    host_port
                }
            });
        Self {
            id: id.to_string(),
            origin,
        }
    }
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| "not base64url".to_string())
}

fn client_data(json: &[u8], kind: &str, challenge: &str, rp: &RelyingParty) -> Result<(), String> {
    let data: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| "the client data is not JSON")?;
    if data["type"] != kind {
        return Err(format!("the client data is not of type {kind}"));
    }
    if data["challenge"].as_str() != Some(challenge) {
        return Err("the challenge does not match".into());
    }
    if data["origin"].as_str() != Some(rp.origin.as_str()) {
        return Err(format!(
            "the origin is {}, not {}",
            data["origin"], rp.origin
        ));
    }
    if data["crossOrigin"] == true {
        return Err("a passkey from another site's frame".into());
    }
    Ok(())
}

/// The parts of authenticator data Ortus uses.
struct AuthData {
    rp_id_hash: [u8; 32],
    flags: u8,
    count: u32,
    /// The credential id and public key (registration only).
    attested: Option<(Vec<u8>, Vec<u8>)>,
}

const UP: u8 = 0x01;
const UV: u8 = 0x04;
const AT: u8 = 0x40;

fn auth_data(bytes: &[u8]) -> Result<AuthData, String> {
    if bytes.len() < 37 {
        return Err("the authenticator data is too short".into());
    }
    let mut rp_id_hash = [0u8; 32];
    rp_id_hash.copy_from_slice(&bytes[..32]);
    let flags = bytes[32];
    let count = u32::from_be_bytes([bytes[33], bytes[34], bytes[35], bytes[36]]);
    let attested = if flags & AT != 0 {
        let rest = &bytes[37..];
        if rest.len() < 18 {
            return Err("the attested credential data is too short".into());
        }
        let length = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
        let id = rest
            .get(18..18 + length)
            .ok_or("the credential id is cut off")?
            .to_vec();
        let key_bytes = &rest[18 + length..];
        let (_, used) = cbor::read(key_bytes)?;
        Some((id, key_bytes[..used].to_vec()))
    } else {
        None
    };
    Ok(AuthData {
        rp_id_hash,
        flags,
        count,
        attested,
    })
}

fn check_flags(data: &AuthData, rp: &RelyingParty, need_uv: bool) -> Result<(), String> {
    if data.rp_id_hash[..] != Sha256::digest(rp.id.as_bytes())[..] {
        return Err("the passkey is for another site".into());
    }
    if data.flags & UP == 0 {
        return Err("the person was not present".into());
    }
    if need_uv && data.flags & UV == 0 {
        return Err("the person was not verified (PIN or biometrics)".into());
    }
    Ok(())
}

/// The COSE algorithms Ortus takes, in the order it prefers them.
pub const ALGORITHMS: [i64; 3] = [-7, -8, -257];

/// Checks that a COSE key is one Ortus can use, and returns its algorithm.
pub fn key_algorithm(cose_key: &[u8]) -> Result<i64, String> {
    let (key, _) = cbor::read(cose_key)?;
    let kty = key
        .int_key(1)
        .and_then(Cbor::as_int)
        .ok_or("the key has no type")?;
    let alg = key
        .int_key(3)
        .and_then(Cbor::as_int)
        .ok_or("the key has no algorithm")?;
    let ok = match (kty, alg) {
        (2, -7) => {
            key.int_key(-1).and_then(Cbor::as_int) == Some(1)
                && key
                    .int_key(-2)
                    .and_then(Cbor::as_bytes)
                    .is_some_and(|x| x.len() == 32)
                && key
                    .int_key(-3)
                    .and_then(Cbor::as_bytes)
                    .is_some_and(|y| y.len() == 32)
        }
        (1, -8) => {
            key.int_key(-1).and_then(Cbor::as_int) == Some(6)
                && key
                    .int_key(-2)
                    .and_then(Cbor::as_bytes)
                    .is_some_and(|x| x.len() == 32)
        }
        (3, -257) => {
            key.int_key(-1)
                .and_then(Cbor::as_bytes)
                .is_some_and(|n| n.len() >= 256)
                && key.int_key(-2).and_then(Cbor::as_bytes).is_some()
        }
        _ => false,
    };
    if ok {
        Ok(alg as i64)
    } else {
        Err(format!(
            "keys of type {kty} with algorithm {alg} are not supported"
        ))
    }
}

/// Whether `signature` over `message` is good for the COSE key.
pub fn verify(cose_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok((key, _)) = cbor::read(cose_key) else {
        return false;
    };
    match key.int_key(3).and_then(Cbor::as_int) {
        Some(-7) => {
            let (Some(x), Some(y)) = (
                key.int_key(-2).and_then(Cbor::as_bytes),
                key.int_key(-3).and_then(Cbor::as_bytes),
            ) else {
                return false;
            };
            let mut point = vec![0x04];
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
                .verify(message, signature)
                .is_ok()
        }
        Some(-8) => key.int_key(-2).and_then(Cbor::as_bytes).is_some_and(|x| {
            UnparsedPublicKey::new(&ED25519, x)
                .verify(message, signature)
                .is_ok()
        }),
        Some(-257) => {
            let (Some(n), Some(e)) = (
                key.int_key(-1).and_then(Cbor::as_bytes),
                key.int_key(-2).and_then(Cbor::as_bytes),
            ) else {
                return false;
            };
            RsaPublicKeyComponents { n, e }
                .verify(&RSA_PKCS1_2048_8192_SHA256, message, signature)
                .is_ok()
        }
        _ => false,
    }
}

/// A passkey just registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub credential_id: Vec<u8>,
    pub cose_key: Vec<u8>,
    pub algorithm: i64,
    pub count: u32,
    pub verified: bool,
}

/// Checks a registration (`navigator.credentials.create`).
pub fn register(
    rp: &RelyingParty,
    challenge: &str,
    client_data_json: &[u8],
    attestation_object: &[u8],
    need_uv: bool,
) -> Result<Registered, String> {
    client_data(client_data_json, "webauthn.create", challenge, rp)?;
    let (attestation, _) = cbor::read(attestation_object)?;
    let auth_bytes = attestation
        .text_key("authData")
        .and_then(Cbor::as_bytes)
        .ok_or("the attestation has no authenticator data")?;
    let data = auth_data(auth_bytes)?;
    check_flags(&data, rp, need_uv)?;
    let (credential_id, cose_key) = data
        .attested
        .ok_or("the authenticator data has no credential")?;
    if credential_id.is_empty() || credential_id.len() > 1023 {
        return Err("the credential id has a bad length".into());
    }
    let algorithm = key_algorithm(&cose_key)?;
    Ok(Registered {
        credential_id,
        cose_key,
        algorithm,
        count: data.count,
        verified: data.flags & UV != 0,
    })
}

/// Checks a sign-in (`navigator.credentials.get`) against the stored key
/// and counter, and returns the new counter.
/// What the browser sends back from `navigator.credentials.get`.
pub struct Assertion<'a> {
    pub client_data_json: &'a [u8],
    pub authenticator_data: &'a [u8],
    pub signature: &'a [u8],
}

pub fn sign_in(
    rp: &RelyingParty,
    challenge: &str,
    assertion: &Assertion<'_>,
    cose_key: &[u8],
    stored_count: u32,
    need_uv: bool,
) -> Result<u32, String> {
    let Assertion {
        client_data_json,
        authenticator_data,
        signature,
    } = *assertion;
    client_data(client_data_json, "webauthn.get", challenge, rp)?;
    let data = auth_data(authenticator_data)?;
    check_flags(&data, rp, need_uv)?;
    let mut message = authenticator_data.to_vec();
    message.extend_from_slice(&Sha256::digest(client_data_json));
    if !verify(cose_key, &message, signature) {
        return Err("the signature does not match".into());
    }
    // A counter that does not move forward means a copied key; zero means
    // the authenticator does not count (as synced passkeys do).
    if (data.count != 0 || stored_count != 0) && data.count <= stored_count {
        return Err("the signature counter went back: the passkey may have been copied".into());
    }
    Ok(data.count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

    /// A software authenticator with one P-256 key.
    struct Authenticator {
        pair: EcdsaKeyPair,
        count: u32,
    }

    impl Authenticator {
        fn new() -> Self {
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                    .unwrap();
            Self {
                pair: EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref())
                    .unwrap(),
                count: 0,
            }
        }

        fn cose(&self) -> Vec<u8> {
            let point = self.pair.public_key().as_ref();
            let mut out = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20];
            out.extend_from_slice(&point[1..33]);
            out.extend_from_slice(&[0x22, 0x58, 0x20]);
            out.extend_from_slice(&point[33..65]);
            out
        }

        fn auth_data(&mut self, rp_id: &str, flags: u8, with_credential: bool) -> Vec<u8> {
            self.count += 1;
            let mut out = Sha256::digest(rp_id.as_bytes()).to_vec();
            out.push(flags | if with_credential { AT } else { 0 });
            out.extend_from_slice(&self.count.to_be_bytes());
            if with_credential {
                out.extend_from_slice(&[0u8; 16]);
                out.extend_from_slice(&[0, 4, 9, 9, 9, 9]);
                out.extend_from_slice(&self.cose());
            }
            out
        }

        fn sign(&self, message: &[u8]) -> Vec<u8> {
            self.pair
                .sign(&SystemRandom::new(), message)
                .unwrap()
                .as_ref()
                .to_vec()
        }
    }

    fn client(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
        format!(r#"{{"type":"{kind}","challenge":"{challenge}","origin":"{origin}","crossOrigin":false}}"#).into_bytes()
    }

    fn attestation(auth_data: &[u8]) -> Vec<u8> {
        // {"fmt": "none", "attStmt": {}, "authData": <bytes>}
        let mut out = vec![
            0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e', 0x67,
        ];
        out.extend_from_slice(b"attStmt");
        out.push(0xa0);
        out.push(0x68);
        out.extend_from_slice(b"authData");
        out.extend_from_slice(&[0x59, (auth_data.len() >> 8) as u8, auth_data.len() as u8]);
        out.extend_from_slice(auth_data);
        out
    }

    #[test]
    fn the_relying_party_comes_from_the_public_address() {
        let rp = RelyingParty::from_public_url("https://ortus.example.org/");
        assert_eq!(
            ("ortus.example.org", "https://ortus.example.org"),
            (rp.id.as_str(), rp.origin.as_str())
        );
        let rp = RelyingParty::from_public_url("http://localhost:7979");
        assert_eq!(
            ("localhost", "http://localhost:7979"),
            (rp.id.as_str(), rp.origin.as_str())
        );
    }

    #[test]
    fn a_passkey_registers_and_signs_in_once_per_challenge() {
        let rp = RelyingParty::from_public_url("https://ortus.example.org");
        let mut key = Authenticator::new();
        let data = key.auth_data("ortus.example.org", UP | UV, true);
        let registered = register(
            &rp,
            "c1",
            &client("webauthn.create", "c1", &rp.origin),
            &attestation(&data),
            true,
        )
        .unwrap();
        assert_eq!(
            (-7, vec![9, 9, 9, 9], 1, true),
            (
                registered.algorithm,
                registered.credential_id.clone(),
                registered.count,
                registered.verified
            )
        );

        let sign =
            |key: &mut Authenticator, challenge: &str, origin: &str, rp_id: &str, flags: u8| {
                let data = key.auth_data(rp_id, flags, false);
                let client = client("webauthn.get", challenge, origin);
                let mut message = data.clone();
                message.extend_from_slice(&Sha256::digest(&client));
                (data, client, key.sign(&message))
            };
        let (data, client_json, signature) = sign(&mut key, "c2", &rp.origin, &rp.id, UP | UV);
        assert_eq!(
            Ok(2),
            sign_in(
                &rp,
                "c2",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                1,
                true
            )
        );
        // Each of these is refused.
        assert!(
            sign_in(
                &rp,
                "c3",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                1,
                true
            )
            .is_err(),
            "other challenge"
        );
        assert!(
            sign_in(
                &rp,
                "c2",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                2,
                true
            )
            .is_err(),
            "counter not moving"
        );
        let mut bad = signature.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(
            sign_in(
                &rp,
                "c2",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &bad
                },
                &registered.cose_key,
                1,
                true
            )
            .is_err(),
            "bad signature"
        );
        let (data, client_json, signature) =
            sign(&mut key, "c4", "https://evil.example", &rp.id, UP | UV);
        assert!(
            sign_in(
                &rp,
                "c4",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                0,
                true
            )
            .is_err(),
            "other origin"
        );
        let (data, client_json, signature) =
            sign(&mut key, "c5", &rp.origin, "evil.example", UP | UV);
        assert!(
            sign_in(
                &rp,
                "c5",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                0,
                true
            )
            .is_err(),
            "other rp id"
        );
        let (data, client_json, signature) = sign(&mut key, "c6", &rp.origin, &rp.id, UP);
        assert!(
            sign_in(
                &rp,
                "c6",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                0,
                true
            )
            .is_err(),
            "not verified"
        );
        assert!(
            sign_in(
                &rp,
                "c6",
                &Assertion {
                    client_data_json: &client_json,
                    authenticator_data: &data,
                    signature: &signature
                },
                &registered.cose_key,
                0,
                false
            )
            .is_ok(),
            "presence is enough when allowed"
        );
    }

    #[test]
    fn registration_checks_the_ceremony() {
        let rp = RelyingParty::from_public_url("https://ortus.example.org");
        let mut key = Authenticator::new();
        let data = key.auth_data("ortus.example.org", UP, true);
        assert!(
            register(
                &rp,
                "c1",
                &client("webauthn.get", "c1", &rp.origin),
                &attestation(&data),
                false
            )
            .is_err(),
            "wrong type"
        );
        assert!(
            register(
                &rp,
                "c1",
                &client("webauthn.create", "c0", &rp.origin),
                &attestation(&data),
                false
            )
            .is_err(),
            "wrong challenge"
        );
        assert!(
            register(
                &rp,
                "c1",
                &client("webauthn.create", "c1", &rp.origin),
                &attestation(&data),
                true
            )
            .is_err(),
            "needs verification"
        );
        let data = key.auth_data("ortus.example.org", 0, true);
        assert!(
            register(
                &rp,
                "c1",
                &client("webauthn.create", "c1", &rp.origin),
                &attestation(&data),
                false
            )
            .is_err(),
            "not present"
        );
    }
}
