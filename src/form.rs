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
