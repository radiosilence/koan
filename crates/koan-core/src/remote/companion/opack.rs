//! OPACK, Apple's binary encoding for the Companion protocol's messages.
//! Enough of it for what is sent and what comes back: no dates, and packing
//! writes no back-references, which readers accept.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Uuid([u8; 16]),
    Array(Vec<Value>),
    Dict(Vec<(Value, Value)>),
}

impl Value {
    pub fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
        Value::Dict(
            entries
                .into_iter()
                .map(|(k, v)| (Value::Str(k.into()), v))
                .collect(),
        )
    }

    /// The value under the string key `key` of a dictionary.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.into())
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<Vec<u8>> for Value {
    fn from(b: Vec<u8>) -> Self {
        Value::Bytes(b)
    }
}

pub fn pack(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    pack_into(value, &mut out);
    out
}

fn sized(out: &mut Vec<u8>, short: u8, long: u8, len: usize, max_short: usize) {
    if len <= max_short {
        out.push(short + len as u8);
    } else if len <= 0xFF {
        out.push(long + 1);
        out.push(len as u8);
    } else if len <= 0xFFFF {
        out.push(long + 2);
        out.extend_from_slice(&(len as u16).to_le_bytes());
    } else {
        out.push(long + 3);
        out.extend_from_slice(&(len as u32).to_le_bytes());
    }
}

fn pack_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(0x04),
        Value::Bool(b) => out.push(if *b { 0x01 } else { 0x02 }),
        Value::Uuid(u) => {
            out.push(0x05);
            out.extend_from_slice(u);
        }
        Value::Int(i) => {
            let i = *i;
            if (0..0x28).contains(&i) {
                out.push(i as u8 + 8);
            } else if (0..=0xFF).contains(&i) {
                out.push(0x30);
                out.push(i as u8);
            } else if (0..=0xFFFF).contains(&i) {
                out.push(0x31);
                out.extend_from_slice(&(i as u16).to_le_bytes());
            } else if (0..=0xFFFF_FFFF).contains(&i) {
                out.push(0x32);
                out.extend_from_slice(&(i as u32).to_le_bytes());
            } else {
                out.push(0x33);
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
        Value::Float(f) => {
            out.push(0x36);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Str(s) => {
            sized(out, 0x40, 0x60, s.len(), 0x20);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Bytes(b) => {
            sized(out, 0x70, 0x90, b.len(), 0x20);
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            out.push(0xD0 + items.len().min(0xF) as u8);
            for item in items {
                pack_into(item, out);
            }
            if items.len() >= 0xF {
                out.push(0x03);
            }
        }
        Value::Dict(entries) => {
            out.push(0xE0 + entries.len().min(0xF) as u8);
            for (k, v) in entries {
                pack_into(k, out);
                pack_into(v, out);
            }
            if entries.len() >= 0xF {
                out.push(0x03);
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("malformed OPACK at byte {0}")]
pub struct Malformed(usize);

pub fn unpack(data: &[u8]) -> Result<Value, Malformed> {
    let mut r = Reader {
        data,
        pos: 0,
        seen: Vec::new(),
    };
    r.value()
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Values a later back-reference can name, in the order read.
    seen: Vec<Value>,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Malformed> {
        let end = self.pos.checked_add(n).ok_or(Malformed(self.pos))?;
        let s = self.data.get(self.pos..end).ok_or(Malformed(self.pos))?;
        self.pos = end;
        Ok(s)
    }

    fn uint(&mut self, n: usize) -> Result<u64, Malformed> {
        let mut buf = [0u8; 8];
        buf[..n].copy_from_slice(self.take(n)?);
        Ok(u64::from_le_bytes(buf))
    }

    fn value(&mut self) -> Result<Value, Malformed> {
        let at = self.pos;
        let tag = self.take(1)?[0];
        let (value, remembered) = match tag {
            0x01 => (Value::Bool(true), false),
            0x02 => (Value::Bool(false), false),
            0x04 => (Value::Null, false),
            0x05 => (Value::Uuid(self.take(16)?.try_into().expect("16")), true),
            0x06 => (Value::Int(self.uint(8)? as i64), true),
            0x08..=0x2F => (Value::Int(tag as i64 - 8), false),
            0x30..=0x33 => (Value::Int(self.uint(1 << (tag & 0xF))? as i64), true),
            0x35 => (
                Value::Float(f32::from_le_bytes(self.take(4)?.try_into().expect("4")) as f64),
                true,
            ),
            0x36 => (
                Value::Float(f64::from_le_bytes(self.take(8)?.try_into().expect("8"))),
                true,
            ),
            0x40..=0x60 => {
                let s = self.take((tag - 0x40) as usize)?;
                (Value::Str(String::from_utf8_lossy(s).into_owned()), true)
            }
            0x61..=0x64 => {
                let len = self.uint((tag & 0xF) as usize)? as usize;
                let s = self.take(len)?;
                (Value::Str(String::from_utf8_lossy(s).into_owned()), true)
            }
            0x70..=0x90 => (
                Value::Bytes(self.take((tag - 0x70) as usize)?.to_vec()),
                true,
            ),
            0x91..=0x94 => {
                let len = self.uint(1 << ((tag & 0xF) - 1))? as usize;
                (Value::Bytes(self.take(len)?.to_vec()), true)
            }
            0xA0..=0xC0 => (self.reference((tag - 0xA0) as usize, at)?, false),
            0xC1..=0xC4 => {
                let index = self.uint((tag - 0xC0) as usize)? as usize;
                (self.reference(index, at)?, false)
            }
            0xD0..=0xDF => {
                let mut items = Vec::new();
                self.each(tag & 0xF, |r| {
                    items.push(r.value()?);
                    Ok(())
                })?;
                (Value::Array(items), false)
            }
            0xE0..=0xEF => {
                let mut entries = Vec::new();
                self.each(tag & 0xF, |r| {
                    let k = r.value()?;
                    entries.push((k, r.value()?));
                    Ok(())
                })?;
                (Value::Dict(entries), false)
            }
            _ => return Err(Malformed(at)),
        };
        if remembered && !self.seen.contains(&value) {
            self.seen.push(value.clone());
        }
        Ok(value)
    }

    fn reference(&self, index: usize, at: usize) -> Result<Value, Malformed> {
        self.seen.get(index).cloned().ok_or(Malformed(at))
    }

    /// `count` items, or items up to a terminator when it is 0xF.
    fn each(
        &mut self,
        count: u8,
        mut item: impl FnMut(&mut Self) -> Result<(), Malformed>,
    ) -> Result<(), Malformed> {
        if count == 0xF {
            while *self.data.get(self.pos).ok_or(Malformed(self.pos))? != 0x03 {
                item(self)?;
            }
            self.pos += 1;
        } else {
            for _ in 0..count {
                item(self)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_packs_as_pyatv_packs_it() {
        // pyatv: opack.pack({"_i": "_launchApp", "_t": 2, "_c": {"_bundleID": "cc.blit.koan"}, "_x": 300})
        let msg = Value::dict([
            ("_i", "_launchApp".into()),
            ("_t", 2.into()),
            ("_c", Value::dict([("_bundleID", "cc.blit.koan".into())])),
            ("_x", 300.into()),
        ]);
        let expected = [
            &[0xE4, 0x42][..],
            b"_i",
            &[0x4A],
            b"_launchApp",
            &[0x42],
            b"_t",
            &[0x0A, 0x42],
            b"_c",
            &[0xE1, 0x49],
            b"_bundleID",
            &[0x4C],
            b"cc.blit.koan",
            &[0x42],
            b"_x",
            &[0x31, 0x2C, 0x01],
        ]
        .concat();
        assert_eq!(pack(&msg), expected);
        assert_eq!(unpack(&expected).unwrap(), msg);
    }

    #[test]
    fn long_values_round_trip() {
        let msg = Value::Array(vec![
            Value::Bytes(vec![7; 300]),
            Value::Str("x".repeat(40)),
            Value::Int(-1),
            Value::Int(70_000),
            Value::Float(1.5),
            Value::Bool(false),
            Value::Null,
            Value::Uuid([9; 16]),
        ]);
        assert_eq!(unpack(&pack(&msg)).unwrap(), msg);
    }

    #[test]
    fn many_entries_are_terminated() {
        let msg = Value::Dict(
            (0..20)
                .map(|i| (Value::Int(i), Value::Bool(true)))
                .collect(),
        );
        let packed = pack(&msg);
        assert_eq!(packed[0], 0xEF);
        assert_eq!(*packed.last().unwrap(), 0x03);
        assert_eq!(unpack(&packed).unwrap(), msg);
    }

    #[test]
    fn a_back_reference_reads_as_the_value_it_names() {
        // ["abc", "abc"], the second written as a reference to the first.
        let data = [0xD2, 0x43, b'a', b'b', b'c', 0xA0];
        assert_eq!(
            unpack(&data).unwrap(),
            Value::Array(vec!["abc".into(), "abc".into()])
        );
    }

    #[test]
    fn truncated_input_is_refused() {
        assert!(unpack(&[0x4A, b'a']).is_err());
        assert!(unpack(&[0xE1, 0x41, b'a']).is_err());
    }
}
