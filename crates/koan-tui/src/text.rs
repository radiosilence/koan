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
    fn lines_wrap_at_spaces_within_the_width() {
        let wrapped = wrap("the quick brown fox jumps", 10);
        assert!(wrapped.iter().all(|l| width(l) <= 10), "{wrapped:?}");
        assert_eq!(wrapped.join(" "), "the quick brown fox jumps");
        assert_eq!(wrap("", 10), Vec::<String>::new());
    }
}
