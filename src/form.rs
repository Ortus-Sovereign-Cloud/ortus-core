//! `application/x-www-form-urlencoded` text: query strings and form
//! bodies.

/// Every `name=value` pair, decoded.
pub fn pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// The first value of `name` in `text`.
pub fn get(text: &str, name: &str) -> Option<String> {
    pairs(text)
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// The value of `name` in the query string of `uri`.
pub fn query(uri: &str, name: &str) -> Option<String> {
    get(uri.split_once('?')?.1.split('#').next()?, name)
}

/// Percent-decoding, with `+` as a space. Works on bytes, so raw UTF-8
/// after a `%` cannot make it panic.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encoding for a query value.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_safely_and_round_trips() {
        assert_eq!(
            Some("auth.".to_string()),
            query("/x?action=auth.&limit=5", "action")
        );
        assert_eq!(Some("a b/c".to_string()), query("/x?q=a+b%2Fc", "q"));
        assert_eq!(Some("%é".to_string()), query("/x?q=%é", "q"));
        assert_eq!(Some("%".to_string()), query("/x?q=%", "q"));
        assert_eq!(None, query("/x", "q"));
        let text = "https://app.example/cb?x=1&y=é";
        assert_eq!(text, decode(&encode(text)));
    }
}

/// A return address after sign-in, if it is safe: a path on this site.
/// Refuses `//host` and `/\\host`, and any control character or backslash
/// anywhere (browsers drop tabs and newlines, so `/\t/host` would become
/// `//host`).
pub fn safe_return(value: &str) -> Option<String> {
    let ok = value.starts_with('/')
        && !value.starts_with("//")
        && !value.chars().any(|c| c.is_control() || c == '\\');
    ok.then(|| value.to_string())
}

#[cfg(test)]
mod safe_return_tests {
    use super::safe_return;

    #[test]
    fn only_paths_on_this_site() {
        assert_eq!(
            Some("/mail/inbox?x=1".into()),
            safe_return("/mail/inbox?x=1")
        );
        for bad in [
            "//evil.com",
            "/\\evil.com",
            "/\t/evil.com",
            "/\n/evil.com",
            "https://evil.com",
            "evil",
            "/a\\b",
            "",
        ] {
            assert_eq!(None, safe_return(bad), "{bad:?}");
        }
    }
}

/// The host (without port) of an `http`/`https` address and its scheme,
/// lowercased, or None when it is not one.
fn web_parts(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        format!("[{}", v6.split(']').next().unwrap_or_default()) + "]"
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };
    Some((scheme, host.to_ascii_lowercase()))
}

/// An address an owner may configure for a server-to-server or browser
/// trip: HTTPS anywhere, or plain HTTP to this machine only (tests).
pub fn web_address_ok(url: &str) -> bool {
    match web_parts(url) {
        Some((scheme, host)) => {
            !host.is_empty()
                && (scheme == "https"
                    || matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]"))
        }
        None => false,
    }
}

/// A return address for an app signing in with Ortus: a web address as
/// above, or an app's own scheme (`com.example.app:/callback`), never one
/// a browser would run or read (`javascript:`, `data:`, `file:`...).
pub fn redirect_uri_ok(uri: &str) -> bool {
    if web_address_ok(uri) {
        return true;
    }
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    let well_formed = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase())
        && scheme
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'));
    well_formed
        && !matches!(
            scheme.as_str(),
            "http" | "https" | "javascript" | "data" | "file" | "vbscript" | "blob" | "about"
        )
}

#[cfg(test)]
mod address_tests {
    use super::{redirect_uri_ok, web_address_ok};

    #[test]
    fn addresses_are_parsed_not_prefixed() {
        for good in [
            "https://idp.example.org/realms/x",
            "HTTPS://idp.example.org",
            "http://localhost:8080/x",
            "http://127.0.0.1:58080",
        ] {
            assert!(web_address_ok(good), "{good}");
        }
        for bad in [
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com/",
            "http://idp.example.org",
            "https://user@evil.com",
            "javascript://x",
            "https://",
            "ftp://x",
        ] {
            assert!(!web_address_ok(bad), "{bad}");
        }
        assert!(redirect_uri_ok("com.example.app:/callback"));
        for bad in [
            "javascript://%0aalert(1)",
            "JAVASCRIPT:alert(1)",
            "data:text/html,x",
            "HTTP://evil.com",
            "http://localhost.evil.com/cb",
        ] {
            assert!(!redirect_uri_ok(bad), "{bad}");
        }
    }
}
