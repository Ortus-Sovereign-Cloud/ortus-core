//! A small CBOR reader (RFC 8949), enough for WebAuthn: unsigned and
//! negative integers, byte and text strings, arrays, maps and simple
//! values. No floats, tags or indefinite lengths; those are refused.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cbor {
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Bool(bool),
    Null,
}

impl Cbor {
    /// The value under `key` in a map with integer keys (COSE keys).
    pub fn int_key(&self, key: i128) -> Option<&Cbor> {
        match self {
            Cbor::Map(entries) => entries
                .iter()
                .find(|(k, _)| *k == Cbor::Int(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value under `key` in a map with text keys.
    pub fn text_key(&self, key: &str) -> Option<&Cbor> {
        match self {
            Cbor::Map(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Cbor::Text(t) if t == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Cbor::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            Cbor::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Cbor::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// Reads one value from the start of `input`; returns it and how many
/// bytes it took.
pub fn read(input: &[u8]) -> Result<(Cbor, usize), String> {
    let mut reader = Reader {
        input,
        at: 0,
        depth: 0,
    };
    let value = reader.value()?;
    Ok((value, reader.at))
}

struct Reader<'a> {
    input: &'a [u8],
    at: usize,
    depth: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.input.get(self.at).ok_or("CBOR ends early")?;
        self.at += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.input.len())
            .ok_or("CBOR ends early")?;
        let slice = &self.input[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn argument(&mut self, info: u8) -> Result<u64, String> {
        Ok(match info {
            0..=23 => u64::from(info),
            24 => u64::from(self.byte()?),
            25 => u64::from(u16::from_be_bytes(
                self.take(2)?.try_into().map_err(|_| "bad CBOR")?,
            )),
            26 => u64::from(u32::from_be_bytes(
                self.take(4)?.try_into().map_err(|_| "bad CBOR")?,
            )),
            27 => u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| "bad CBOR")?),
            _ => return Err("indefinite or reserved CBOR lengths are not supported".into()),
        })
    }

    fn value(&mut self) -> Result<Cbor, String> {
        self.depth += 1;
        if self.depth > 16 {
            return Err("CBOR nests too deep".into());
        }
        let first = self.byte()?;
        let (major, info) = (first >> 5, first & 0x1f);
        let value = match major {
            0 => Cbor::Int(i128::from(self.argument(info)?)),
            1 => Cbor::Int(-1 - i128::from(self.argument(info)?)),
            2 => {
                let n = usize::try_from(self.argument(info)?).map_err(|_| "CBOR too long")?;
                Cbor::Bytes(self.take(n)?.to_vec())
            }
            3 => {
                let n = usize::try_from(self.argument(info)?).map_err(|_| "CBOR too long")?;
                Cbor::Text(
                    String::from_utf8(self.take(n)?.to_vec())
                        .map_err(|_| "CBOR text is not UTF-8")?,
                )
            }
            4 => {
                let n = self.argument(info)?;
                if n > 1024 {
                    return Err("CBOR array too long".into());
                }
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(self.value()?);
                }
                Cbor::Array(items)
            }
            5 => {
                let n = self.argument(info)?;
                if n > 1024 {
                    return Err("CBOR map too long".into());
                }
                let mut entries = Vec::new();
                for _ in 0..n {
                    let key = self.value()?;
                    entries.push((key, self.value()?));
                }
                Cbor::Map(entries)
            }
            7 => match info {
                20 => Cbor::Bool(false),
                21 => Cbor::Bool(true),
                22 => Cbor::Null,
                _ => return Err("CBOR floats and other simple values are not supported".into()),
            },
            _ => return Err("CBOR tags are not supported".into()),
        };
        self.depth -= 1;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_rfc_examples() {
        assert_eq!(Cbor::Int(0), read(&[0x00]).unwrap().0);
        assert_eq!(Cbor::Int(500), read(&[0x19, 0x01, 0xf4]).unwrap().0);
        assert_eq!(Cbor::Int(-7), read(&[0x26]).unwrap().0);
        assert_eq!(Cbor::Int(-257), read(&[0x39, 0x01, 0x00]).unwrap().0);
        assert_eq!(
            Cbor::Bytes(vec![1, 2, 3, 4]),
            read(&[0x44, 1, 2, 3, 4]).unwrap().0
        );
        assert_eq!(
            Cbor::Text("IETF".into()),
            read(&[0x64, 0x49, 0x45, 0x54, 0x46]).unwrap().0
        );
        let map = read(&[0xa2, 0x01, 0x02, 0x03, 0x04]).unwrap().0;
        assert_eq!(Some(&Cbor::Int(4)), map.int_key(3));
        assert_eq!(
            (Cbor::Array(vec![Cbor::Int(1), Cbor::Null]), 3),
            read(&[0x82, 0x01, 0xf6]).unwrap()
        );
    }

    #[test]
    fn refuses_what_it_does_not_read() {
        assert!(read(&[0x5f]).is_err(), "indefinite length");
        assert!(read(&[0xf9, 0x3c, 0x00]).is_err(), "float");
        assert!(read(&[0x44, 1, 2]).is_err(), "short");
        assert!(read(&[0xc1, 0x00]).is_err(), "tag");
        let deep = vec![0x81; 40];
        assert!(read(&deep).is_err(), "too deep");
    }
}
