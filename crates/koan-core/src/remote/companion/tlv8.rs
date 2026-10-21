//! TLV8, HomeKit's pairing encoding: a type byte, a length byte, the value,
//! with values over 255 bytes split across consecutive items of one type.

pub const METHOD: u8 = 0x00;
pub const IDENTIFIER: u8 = 0x01;
pub const SALT: u8 = 0x02;
pub const PUBLIC_KEY: u8 = 0x03;
pub const PROOF: u8 = 0x04;
pub const ENCRYPTED_DATA: u8 = 0x05;
pub const STATE: u8 = 0x06;
pub const ERROR: u8 = 0x07;
pub const BACK_OFF: u8 = 0x08;
pub const SIGNATURE: u8 = 0x0A;
pub const NAME: u8 = 0x11;

pub fn write(items: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, value) in items {
        if value.is_empty() {
            out.extend_from_slice(&[*tag, 0]);
        }
        for chunk in value.chunks(255) {
            out.push(*tag);
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
    }
    out
}

/// The items of `data`, a value split over several joined back together.
/// `None` when the data runs short.
pub fn read(data: &[u8]) -> Option<Vec<(u8, Vec<u8>)>> {
    let mut out: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut pos = 0;
    let mut last = None;
    while pos < data.len() {
        let tag = data[pos];
        let len = *data.get(pos + 1)? as usize;
        let value = data.get(pos + 2..pos + 2 + len)?;
        match out.last_mut() {
            Some((t, v)) if last == Some(tag) && *t == tag => v.extend_from_slice(value),
            _ => out.push((tag, value.to_vec())),
        }
        last = Some(tag);
        pos += 2 + len;
    }
    Some(out)
}

pub fn find(items: &[(u8, Vec<u8>)], tag: u8) -> Option<&[u8]> {
    items
        .iter()
        .find(|(t, _)| *t == tag)
        .map(|(_, v)| v.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_value_is_split_and_joined() {
        let long = vec![1u8; 300];
        let data = write(&[(STATE, &[3]), (PUBLIC_KEY, &long), (PROOF, &[9; 4])]);
        assert_eq!(&data[..3], &[STATE, 1, 3]);
        assert_eq!(&data[3..5], &[PUBLIC_KEY, 255]);
        assert_eq!(&data[260..262], &[PUBLIC_KEY, 45]);
        let items = read(&data).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(find(&items, PUBLIC_KEY).unwrap(), &long[..]);
        assert_eq!(find(&items, PROOF).unwrap(), &[9; 4]);
    }

    #[test]
    fn a_short_read_is_refused() {
        assert!(read(&[STATE, 4, 1]).is_none());
    }
}
