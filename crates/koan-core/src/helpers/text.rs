//! String and path utilities.

use std::path::Path;

/// Fisher-Yates over a fresh seed, so consecutive calls differ.
///
/// Deliberately not seeded from anything stable: "shuffle again" has to
/// actually produce a new order, which a process-lifetime seed wouldn't.
pub fn shuffle<T>(items: &mut [T]) {
    let Some(mut rng) = Rng::seeded() else {
        return; // Leave the order alone rather than pretending to shuffle.
    };
    for i in (1..items.len()).rev() {
        items.swap(i, rng.below(i + 1));
    }
}

/// xorshift64 over a fresh seed — plenty for shuffling a list nobody is
/// betting on.
pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Option<Self> {
        let mut seed = [0u8; 8];
        getrandom::fill(&mut seed).ok()?;
        Some(Self(u64::from_le_bytes(seed) | 1))
    }

    /// A number in `0..n`. `n` must not be zero.
    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Truncate a string to at most `max` bytes, cutting on a char boundary.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
/// `.` and `..` become `_`: tags and server metadata are untrusted, and either
/// would move the path out of the directory it is joined onto.
pub fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    let cleaned = truncate_bytes(&cleaned, 240).trim_end().to_string();
    match cleaned.as_str() {
        "." | ".." => "_".into(),
        _ => cleaned,
    }
}

/// A file extension from a codec name, which for remote tracks is whatever
/// the server sent as `suffix`. ASCII alphanumerics only, so it can never
/// carry a separator or a `..`; `None` when nothing usable is left.
pub fn sanitise_extension(codec: &str) -> Option<String> {
    let ext: String = codec
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(16)
        .collect::<String>()
        .to_lowercase();
    (!ext.is_empty()).then_some(ext)
}

/// Whether `path` lies inside `dir` without leaving it on the way — every
/// component after the prefix is a plain name.
pub fn path_within(dir: &Path, path: &Path) -> bool {
    path.strip_prefix(dir).is_ok_and(|rest| {
        rest.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    })
}

/// The year a tag date starts with. `get`, not a slice: a date is free text,
/// and a multibyte character in its first four bytes would panic a slice.
pub fn year_of(date: &str) -> Option<&str> {
    date.get(..4)
}

#[cfg(test)]
mod year_tests {
    use super::year_of;

    #[test]
    fn a_year_is_the_first_four_characters_when_they_are_bytes_too() {
        assert_eq!(year_of("1997-05-21"), Some("1997"));
        assert_eq!(year_of("199"), None);
        // Full-width digits: four bytes in is mid-character.
        assert_eq!(year_of("１９９７"), None);
    }
}
