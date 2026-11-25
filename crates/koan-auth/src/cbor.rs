//! Enough CBOR (RFC 8949) to read what WebAuthn sends: an attestation object
//! and the COSE key inside authenticator data. CTAP2's canonical encoding
//! only: definite lengths, arguments in their shortest form, no duplicate map
//! keys, no tags or floats. Anything else is refused.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Bool(bool),
    Null,
}

impl Value {
    /// The value under `key` in a map.
    pub(crate) fn get(&self, key: &Value) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn int(&self, key: i128) -> Option<&Value> {
        self.get(&Value::Int(key))
    }

    pub(crate) fn text(&self, key: &str) -> Option<&Value> {
        self.get(&Value::Text(key.to_owned()))
    }

    pub(crate) fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub(crate) fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }
}

/// Nesting deeper than any WebAuthn structure, refused so that hostile input
/// cannot run the reader out of stack.
const MAX_DEPTH: usize = 16;

/// The first value in `input`, and how many bytes it took.
pub(crate) fn decode(input: &[u8]) -> Option<(Value, usize)> {
    let mut reader = Reader { input, at: 0 };
    let value = reader.value(0)?;
    Some((value, reader.at))
}

struct Reader<'a> {
    input: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let bytes = self.input.get(self.at..end)?;
        self.at = end;
        Some(bytes)
    }

    /// A head's argument, refused when a shorter form would have held it.
    fn argument(&mut self, info: u8) -> Option<u64> {
        let (value, floor) = match info {
            0..=23 => return Some(info as u64),
            24 => (self.take(1)?[0] as u64, 24),
            25 => (
                u16::from_be_bytes(self.take(2)?.try_into().ok()?) as u64,
                0x100,
            ),
            26 => (
                u32::from_be_bytes(self.take(4)?.try_into().ok()?) as u64,
                0x1_0000,
            ),
            27 => (
                u64::from_be_bytes(self.take(8)?.try_into().ok()?),
                0x1_0000_0000,
            ),
            _ => return None,
        };
        (value >= floor).then_some(value)
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        let head = self.take(1)?[0];
        let (major, info) = (head >> 5, head & 0x1f);
        if major == 7 {
            return match info {
                20 => Some(Value::Bool(false)),
                21 => Some(Value::Bool(true)),
                22 => Some(Value::Null),
                _ => None,
            };
        }
        let arg = self.argument(info)?;
        // A count can be no larger than the bytes left, each item taking one
        // at least, so a forged length cannot make a huge allocation.
        let left = (self.input.len() - self.at) as u64;
        Some(match major {
            0 => Value::Int(arg as i128),
            1 => Value::Int(-1 - arg as i128),
            2 => Value::Bytes(self.take(usize::try_from(arg).ok()?)?.to_vec()),
            3 => Value::Text(
                String::from_utf8(self.take(usize::try_from(arg).ok()?)?.to_vec()).ok()?,
            ),
            4 if arg <= left => {
                let mut items = Vec::with_capacity(arg as usize);
                for _ in 0..arg {
                    items.push(self.value(depth + 1)?);
                }
                Value::Array(items)
            }
            5 if arg <= left => {
                let mut entries = Vec::with_capacity(arg as usize);
                for _ in 0..arg {
                    let k = self.value(depth + 1)?;
                    if entries.iter().any(|(seen, _)| *seen == k) {
                        return None;
                    }
                    let v = self.value(depth + 1)?;
                    entries.push((k, v));
                }
                Value::Map(entries)
            }
            _ => return None,
        })
    }
}

/// CBOR written, for tests that play an authenticator.
#[cfg(test)]
pub(crate) fn encode(value: &Value) -> Vec<u8> {
    fn head(out: &mut Vec<u8>, major: u8, n: u64) {
        let m = major << 5;
        match n {
            0..=23 => out.push(m | n as u8),
            24..=0xff => out.extend([m | 24, n as u8]),
            0x100..=0xffff => {
                out.push(m | 25);
                out.extend((n as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                out.push(m | 26);
                out.extend((n as u32).to_be_bytes());
            }
            _ => {
                out.push(m | 27);
                out.extend(n.to_be_bytes());
            }
        }
    }
    fn write(out: &mut Vec<u8>, value: &Value) {
        match value {
            Value::Int(n) if *n >= 0 => head(out, 0, *n as u64),
            Value::Int(n) => head(out, 1, (-1 - n) as u64),
            Value::Bytes(b) => {
                head(out, 2, b.len() as u64);
                out.extend(b);
            }
            Value::Text(t) => {
                head(out, 3, t.len() as u64);
                out.extend(t.as_bytes());
            }
            Value::Array(items) => {
                head(out, 4, items.len() as u64);
                items.iter().for_each(|v| write(out, v));
            }
            Value::Map(entries) => {
                head(out, 5, entries.len() as u64);
                for (k, v) in entries {
                    write(out, k);
                    write(out, v);
                }
            }
            Value::Bool(b) => out.push(0xf4 | *b as u8),
            Value::Null => out.push(0xf6),
        }
    }
    let mut out = Vec::new();
    write(&mut out, value);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let value = Value::Map(vec![
            (Value::Int(1), Value::Int(2)),
            (Value::Int(-7), Value::Bytes(vec![1, 2, 3])),
            (Value::Text("fmt".into()), Value::Text("none".into())),
            (
                Value::Text("a".into()),
                Value::Array(vec![Value::Bool(true), Value::Null, Value::Int(70000)]),
            ),
        ]);
        let bytes = encode(&value);
        let mut trailing = bytes.clone();
        trailing.extend([0xff, 0xff]);
        assert_eq!(decode(&trailing), Some((value, bytes.len())));
    }

    #[test]
    fn known_encodings() {
        // RFC 8949 Appendix A.
        assert_eq!(decode(&[0x39, 0x01, 0x00]).unwrap().0, Value::Int(-257));
        assert_eq!(decode(&[0x18, 0x64]).unwrap().0, Value::Int(100));
        assert_eq!(
            decode(&[0x63, 0x49, 0x45, 0x54]).unwrap().0,
            Value::Text("IET".into())
        );
    }

    #[test]
    fn hostile_input_is_refused() {
        // Truncated, indefinite length, a huge count, too deep.
        assert_eq!(decode(&[0x42, 0x01]), None);
        assert_eq!(decode(&[0x5f]), None);
        assert_eq!(
            decode(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
            None
        );
        assert_eq!(decode(&[0x81; 64]), None);
        // A duplicate key, and 5 written in two bytes.
        assert_eq!(decode(&[0xa2, 0x01, 0x02, 0x01, 0x03]), None);
        assert_eq!(decode(&[0x18, 0x05]), None);
    }
}
