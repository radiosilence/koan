//! Text measured as a terminal draws it: by the cells each character takes,
//! so a wide character counts twice and a combining one not at all.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The cells `text` takes.
pub fn width(text: &str) -> usize {
    text.width()
}

/// `text` cut to `width` cells, with an ellipsis where it was cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// The end of `text` that fits in `width` cells: what is being typed.
pub fn tail(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut kept: Vec<char> = text
        .chars()
        .rev()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= width
        })
        .collect();
    kept.reverse();
    kept.into_iter().collect()
}

/// `text` broken into lines of at most `width` cells, at spaces where it can.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&fit(word, width));
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text` fitted to exactly `width` cells, padded on the right: a column's
/// cell.
pub fn fit_pad(text: &str, width: usize) -> String {
    let cut = fit(text, width);
    let pad = width - cut.width();
    format!("{cut}{}", " ".repeat(pad))
}

/// `text` fitted to exactly `width` cells, padded on the left: a duration, a
/// count.
pub fn fit_pad_right(text: &str, width: usize) -> String {
    let cut = fit(text, width);
    let pad = width - cut.width();
    format!("{}{cut}", " ".repeat(pad))
}

/// "1 album", "3 albums".
pub fn count(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Bytes as a person reads them.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit + 1 < UNITS.len() {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} bytes")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_characters_count_twice() {
        assert_eq!(fit("音楽のライブラリ", 7), "音楽の\u{2026}");
        assert_eq!(width(&fit("音楽のライブラリ", 7)), 7);
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("anything", 0), "");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("音楽ab", 3), "ab");
    }

    #[test]
    fn padded_cells_are_exactly_their_width() {
        assert_eq!(width(&fit_pad("東京事変", 5)), 5);
        assert_eq!(fit_pad_right("3:07", 6), "  3:07");
        assert_eq!(count(1, "album"), "1 album");
        assert_eq!(count(3, "album"), "3 albums");
        assert_eq!(bytes(999), "999 bytes");
        assert_eq!(bytes(1_500_000), "1.5 MB");
    }

    #[test]
    fn lines_wrap_at_spaces_within_the_width() {
        let wrapped = wrap("the quick brown fox jumps", 10);
        assert!(wrapped.iter().all(|l| width(l) <= 10), "{wrapped:?}");
        assert_eq!(wrapped.join(" "), "the quick brown fox jumps");
        assert_eq!(wrap("", 10), Vec::<String>::new());
    }
}
