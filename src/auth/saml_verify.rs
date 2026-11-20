//! Checking a SAML response, narrowly: the strict rules that keep XML
//! signature wrapping and its relatives out.
//!
//! What is accepted, and nothing else:
//! - no DOCTYPE and no entities;
//! - one `Response` holding exactly one `Assertion` (never encrypted),
//!   and no other `Assertion` anywhere; every `ID` in the document unique;
//! - signatures only as direct children of the `Assertion` and the
//!   `Response` (at most one each); the assertion's is the one checked
//!   when it has one, else the response's; it has exactly one
//!   `Reference`, to its parent's `ID`;
//! - the enveloped-signature and exclusive canonicalisation transforms,
//!   SHA-256 digests, RSA-SHA256 signatures, no comments in the signed
//!   part;
//! - the key the owner configured; any `KeyInfo` in the message is
//!   ignored.
//!
//! Everything read afterwards comes from the verified element only.

use aws_lc_rs::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bergshamra_xml::NodeSet;
use sha2::{Digest, Sha256};
use uppsala::{Document, NodeId, NodeKind};

pub const PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
pub const ASSERTION: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const DSIG: &str = "http://www.w3.org/2000/09/xmldsig#";
const EXC_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const ENVELOPED: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
const RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
const SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";

/// What a verified response says, read from the signed element only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verified {
    pub assertion_id: String,
    pub issuer: String,
    pub name_id: String,
    pub audiences: Vec<String>,
    pub not_before: Option<i64>,
    pub not_on_or_after: Option<i64>,
    pub recipient: Option<String>,
    pub in_response_to: Option<String>,
    pub confirmation_not_on_or_after: Option<i64>,
    /// (attribute name, all its values)
    pub attributes: Vec<(String, Vec<String>)>,
    /// From the Response (not signed when only the assertion is): checked
    /// for consistency, never trusted alone.
    pub response_status: String,
    pub response_destination: Option<String>,
}

/// The RSA public key (DER RSAPublicKey) inside a PEM or base64 X.509
/// certificate.
pub fn rsa_key_of_certificate(text: &str) -> Result<Vec<u8>, String> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = STANDARD
        .decode(body)
        .map_err(|_| "the certificate is not base64".to_string())?;
    // Certificate ::= SEQUENCE { tbsCertificate SEQUENCE { [0] version?,
    // serial, signature, issuer, validity, subject, subjectPublicKeyInfo, ...
    let outer = der_value(&der, 0x30)?;
    let certificate = der_children(&outer)?;
    let tbs = der_children(
        certificate
            .first()
            .filter(|e| e.0 == 0x30)
            .ok_or("no tbsCertificate")?
            .1,
    )?;
    let skip = usize::from(tbs.first().is_some_and(|e| e.0 == 0xa0));
    let spki = tbs
        .get(skip + 5)
        .filter(|e| e.0 == 0x30)
        .ok_or("no subjectPublicKeyInfo")?;
    let parts = der_children(spki.1)?;
    let algorithm = der_children(
        parts
            .first()
            .filter(|e| e.0 == 0x30)
            .ok_or("no algorithm")?
            .1,
    )?;
    // rsaEncryption 1.2.840.113549.1.1.1
    if algorithm.first().map(|e| e.1)
        != Some(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01][..])
    {
        return Err("the certificate's key is not RSA".into());
    }
    let bits = parts
        .get(1)
        .filter(|e| e.0 == 0x03)
        .ok_or("no public key")?
        .1;
    bits.split_first()
        .filter(|(unused, _)| **unused == 0)
        .map(|(_, key)| key.to_vec())
        .ok_or_else(|| "a bad public key".to_string())
}

fn der_value(data: &[u8], tag: u8) -> Result<Vec<u8>, String> {
    let (found, value, _) = der_one(data)?;
    if found != tag {
        return Err("unexpected DER".into());
    }
    Ok(value.to_vec())
}

fn der_one(data: &[u8]) -> Result<(u8, &[u8], usize), String> {
    let tag = *data.first().ok_or("short DER")?;
    let first = *data.get(1).ok_or("short DER")?;
    let (length, start) = if first < 0x80 {
        (usize::from(first), 2)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 {
            return Err("bad DER length".into());
        }
        let bytes = data.get(2..2 + n).ok_or("short DER")?;
        (
            bytes
                .iter()
                .fold(0usize, |acc, b| (acc << 8) | usize::from(*b)),
            2 + n,
        )
    };
    let value = data.get(start..start + length).ok_or("short DER")?;
    Ok((tag, value, start + length))
}

fn der_children(mut data: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let (tag, value, used) = der_one(data)?;
        out.push((tag, value));
        data = &data[used..];
    }
    Ok(out)
}

fn is(doc: &Document<'_>, id: NodeId, namespace: &str, local: &str) -> bool {
    doc.element(id).is_some_and(|e| {
        e.name.local_name == local && e.name.namespace_uri.as_deref() == Some(namespace)
    })
}

fn element_children(doc: &Document<'_>, id: NodeId) -> Vec<NodeId> {
    doc.children(id)
        .into_iter()
        .filter(|c| doc.element(*c).is_some())
        .collect()
}

fn child(doc: &Document<'_>, id: NodeId, namespace: &str, local: &str) -> Option<NodeId> {
    element_children(doc, id)
        .into_iter()
        .find(|c| is(doc, *c, namespace, local))
}

fn only_child(
    doc: &Document<'_>,
    id: NodeId,
    namespace: &str,
    local: &str,
) -> Result<NodeId, String> {
    let found: Vec<NodeId> = element_children(doc, id)
        .into_iter()
        .filter(|c| is(doc, *c, namespace, local))
        .collect();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("no {local}")),
        _ => Err(format!("more than one {local}")),
    }
}

/// All descendants, in document order (elements and others).
fn descendants(doc: &Document<'_>, id: NodeId, out: &mut Vec<NodeId>) {
    for c in doc.children(id) {
        out.push(c);
        descendants(doc, c, out);
    }
}

/// The text of an element: every text node in it, joined (never just the
/// first one, which a comment could cut short).
fn text(doc: &Document<'_>, id: NodeId) -> String {
    let mut all = Vec::new();
    descendants(doc, id, &mut all);
    all.into_iter()
        .filter_map(|n| match doc.node_kind(n) {
            Some(NodeKind::Text(t)) | Some(NodeKind::CData(t)) => Some(t.to_string()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn attribute(doc: &Document<'_>, id: NodeId, name: &str) -> Option<String> {
    doc.element(id)?
        .attributes
        .iter()
        .find(|a| a.name.local_name == name && a.name.namespace_uri.is_none())
        .map(|a| a.value.to_string())
}

/// The prefixes of an InclusiveNamespaces child, if any.
fn inclusive_prefixes(doc: &Document<'_>, id: NodeId) -> Vec<String> {
    element_children(doc, id)
        .into_iter()
        .find(|c| {
            doc.element(*c)
                .is_some_and(|e| e.name.local_name == "InclusiveNamespaces")
        })
        .and_then(|c| attribute(doc, c, "PrefixList"))
        .map(|list| list.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn c14n(doc: &Document<'_>, set: &NodeSet, prefixes: &[String]) -> Result<Vec<u8>, String> {
    bergshamra_c14n::exclusive::canonicalize(doc, false, Some(set), prefixes)
        .map_err(|e| format!("canonicalisation failed: {e:?}"))
}

/// Checks the one signature of `signed` (whose direct child it is) with
/// `rsa_key`.
fn verify_signature(
    doc: &Document<'_>,
    signed: NodeId,
    signature: NodeId,
    rsa_key: &[u8],
) -> Result<(), String> {
    let signed_id = attribute(doc, signed, "ID").ok_or("the signed element has no ID")?;
    let signed_info = only_child(doc, signature, DSIG, "SignedInfo")?;
    let method = only_child(doc, signed_info, DSIG, "CanonicalizationMethod")?;
    if attribute(doc, method, "Algorithm").as_deref() != Some(EXC_C14N) {
        return Err("only exclusive canonicalisation is accepted".into());
    }
    let algorithm = only_child(doc, signed_info, DSIG, "SignatureMethod")?;
    if attribute(doc, algorithm, "Algorithm").as_deref() != Some(RSA_SHA256) {
        return Err("only RSA-SHA256 signatures are accepted".into());
    }
    let reference = only_child(doc, signed_info, DSIG, "Reference")?;
    if attribute(doc, reference, "URI") != Some(format!("#{signed_id}")) {
        return Err("the signature does not refer to the element it is in".into());
    }
    let transforms = only_child(doc, reference, DSIG, "Transforms")?;
    let steps: Vec<NodeId> = element_children(doc, transforms);
    let names: Vec<String> = steps
        .iter()
        .map(|t| attribute(doc, *t, "Algorithm").unwrap_or_default())
        .collect();
    if names != [ENVELOPED, EXC_C14N] {
        return Err(
            "only the enveloped-signature and exclusive canonicalisation transforms are accepted"
                .into(),
        );
    }
    let digest_method = only_child(doc, reference, DSIG, "DigestMethod")?;
    if attribute(doc, digest_method, "Algorithm").as_deref() != Some(SHA256) {
        return Err("only SHA-256 digests are accepted".into());
    }
    let expected = STANDARD
        .decode(
            text(doc, only_child(doc, reference, DSIG, "DigestValue")?)
                .replace(char::is_whitespace, ""),
        )
        .map_err(|_| "a bad digest value")?;

    // No comments anywhere in what is signed.
    let mut inside = Vec::new();
    descendants(doc, signed, &mut inside);
    if inside
        .iter()
        .any(|n| matches!(doc.node_kind(*n), Some(NodeKind::Comment(_))))
    {
        return Err("comments are not accepted in signed content".into());
    }
    let content = NodeSet::tree_without_comments(signed, doc)
        .subtract(&NodeSet::tree_with_comments(signature, doc));
    let digest = Sha256::digest(c14n(doc, &content, &inclusive_prefixes(doc, steps[1]))?);
    if digest[..] != expected[..] {
        return Err("the signed content has changed".into());
    }
    let info = c14n(
        doc,
        &NodeSet::tree_without_comments(signed_info, doc),
        &inclusive_prefixes(doc, method),
    )?;
    let value = STANDARD
        .decode(
            text(doc, only_child(doc, signature, DSIG, "SignatureValue")?)
                .replace(char::is_whitespace, ""),
        )
        .map_err(|_| "a bad signature value")?;
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, rsa_key)
        .verify(&info, &value)
        .map_err(|_| "the signature does not match the provider's key".to_string())
}

/// A SAML time (`2026-11-20T06:00:00Z`, with or without fractions) as
/// Unix seconds.
pub fn parse_time(text: &str) -> Option<i64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.split('.').next()?;
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || min > 59 || s > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + min * 60 + s)
}

/// Unix seconds as a SAML time.
pub fn format_time(seconds: i64) -> String {
    let name = crate::cli::dated_name(seconds * 1000); // 20261120T060000Z
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &name[0..4],
        &name[4..6],
        &name[6..8],
        &name[9..11],
        &name[11..13],
        &name[13..15]
    )
}

/// Checks a SAML response (the XML text) against the provider's RSA key
/// and returns what its signed part says.
pub fn verify(xml: &str, rsa_key: &[u8]) -> Result<Verified, String> {
    let lowered = xml.to_ascii_lowercase();
    if lowered.contains("<!doctype") || lowered.contains("<!entity") {
        return Err("a DOCTYPE or entities are not accepted".into());
    }
    let doc = uppsala::parse(xml).map_err(|e| format!("not XML: {e}"))?;
    let response = doc.document_element().ok_or("empty document")?;
    if !is(&doc, response, PROTOCOL, "Response") {
        return Err("not a SAML Response".into());
    }
    let mut all = vec![response];
    descendants(&doc, response, &mut all);
    let mut ids = std::collections::HashSet::new();
    let mut assertions = 0;
    for node in &all {
        if let Some(element) = doc.element(*node) {
            if element.name.local_name == "EncryptedAssertion" {
                return Err("encrypted assertions are not accepted".into());
            }
            if element.name.local_name == "Assertion" {
                assertions += 1;
            }
            if let Some(id) = attribute(&doc, *node, "ID") {
                if !ids.insert(id) {
                    return Err("an ID appears twice".into());
                }
            }
        }
    }
    if assertions != 1 {
        return Err("there must be exactly one assertion".into());
    }
    let assertion = only_child(&doc, response, ASSERTION, "Assertion")?;
    // The signed element: the assertion if it is signed, else the response.
    let (signed, signature) = match child(&doc, assertion, DSIG, "Signature") {
        Some(signature) => (assertion, signature),
        None => (
            response,
            child(&doc, response, DSIG, "Signature").ok_or("the response is not signed")?,
        ),
    };
    // Signatures only as direct children of the assertion or the
    // response, at most one each; none anywhere else.
    for node in &all {
        if is(&doc, *node, DSIG, "Signature") {
            let parent = doc.parent(*node);
            if parent != Some(assertion) && parent != Some(response) {
                return Err("a signature in an unexpected place".into());
            }
        }
    }
    for holder in [assertion, response] {
        if element_children(&doc, holder)
            .into_iter()
            .filter(|c| is(&doc, *c, DSIG, "Signature"))
            .count()
            > 1
        {
            return Err("more than one signature on one element".into());
        }
    }
    verify_signature(&doc, signed, signature, rsa_key)?;

    // From here, read only from inside the verified element.
    let assertion = if signed == assertion {
        assertion
    } else {
        only_child(&doc, signed, ASSERTION, "Assertion")?
    };
    let mut verified = Verified {
        assertion_id: attribute(&doc, assertion, "ID").unwrap_or_default(),
        issuer: text(&doc, only_child(&doc, assertion, ASSERTION, "Issuer")?),
        ..Default::default()
    };
    let subject = only_child(&doc, assertion, ASSERTION, "Subject")?;
    verified.name_id = text(&doc, only_child(&doc, subject, ASSERTION, "NameID")?);
    for confirmation in element_children(&doc, subject)
        .into_iter()
        .filter(|c| is(&doc, *c, ASSERTION, "SubjectConfirmation"))
    {
        if attribute(&doc, confirmation, "Method").as_deref()
            == Some("urn:oasis:names:tc:SAML:2.0:cm:bearer")
        {
            let data = only_child(&doc, confirmation, ASSERTION, "SubjectConfirmationData")?;
            verified.recipient = attribute(&doc, data, "Recipient");
            verified.in_response_to = attribute(&doc, data, "InResponseTo");
            verified.confirmation_not_on_or_after =
                attribute(&doc, data, "NotOnOrAfter").and_then(|t| parse_time(&t));
        }
    }
    if let Some(conditions) = child(&doc, assertion, ASSERTION, "Conditions") {
        verified.not_before = attribute(&doc, conditions, "NotBefore").and_then(|t| parse_time(&t));
        verified.not_on_or_after =
            attribute(&doc, conditions, "NotOnOrAfter").and_then(|t| parse_time(&t));
        for restriction in element_children(&doc, conditions)
            .into_iter()
            .filter(|c| is(&doc, *c, ASSERTION, "AudienceRestriction"))
        {
            for audience in element_children(&doc, restriction)
                .into_iter()
                .filter(|c| is(&doc, *c, ASSERTION, "Audience"))
            {
                verified.audiences.push(text(&doc, audience));
            }
        }
    }
    for statement in element_children(&doc, assertion)
        .into_iter()
        .filter(|c| is(&doc, *c, ASSERTION, "AttributeStatement"))
    {
        for attr in element_children(&doc, statement)
            .into_iter()
            .filter(|c| is(&doc, *c, ASSERTION, "Attribute"))
        {
            let values = element_children(&doc, attr)
                .into_iter()
                .filter(|c| is(&doc, *c, ASSERTION, "AttributeValue"))
                .map(|v| text(&doc, v))
                .collect();
            verified
                .attributes
                .push((attribute(&doc, attr, "Name").unwrap_or_default(), values));
        }
    }
    verified.response_destination = attribute(&doc, response, "Destination");
    if let Some(status) = child(&doc, response, PROTOCOL, "Status") {
        if let Some(code) = child(&doc, status, PROTOCOL, "StatusCode") {
            verified.response_status = attribute(&doc, code, "Value").unwrap_or_default();
        }
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saml_times_round_trip() {
        assert_eq!(Some(0), parse_time("1970-01-01T00:00:00Z"));
        assert_eq!(Some(1_794_993_300), parse_time("2026-11-18T09:15:00Z"));
        assert_eq!(Some(1_794_993_300), parse_time("2026-11-18T09:15:00.123Z"));
        assert_eq!("2026-11-18T09:15:00Z", format_time(1_794_993_300));
        assert_eq!(None, parse_time("2026-11-18 09:15:00"));
        assert_eq!(None, parse_time("2026-13-18T09:15:00Z"));
    }

    #[test]
    fn doctype_and_entities_are_refused() {
        let error = verify("<!DOCTYPE x [<!ENTITY a 'b'>]><samlp:Response/>", &[]).unwrap_err();
        assert!(error.contains("DOCTYPE"), "{error}");
    }
}
