use std::path::Path;

pub fn call_function(name: &str, args: &[String]) -> Option<String> {
    match name {
        // String functions
        "left" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            Some(s.chars().take(n).collect())
        }
        "right" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            let chars: Vec<char> = s.chars().collect();
            let start = chars.len().saturating_sub(n);
            Some(chars[start..].iter().collect())
        }
        "pad" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            Some(format!("{s:>n$}"))
        }
        "pad_right" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            Some(format!("{s:<n$}"))
        }
        "replace" => {
            let s = args.first()?;
            let from = args.get(1)?;
            let to = args.get(2)?;
            Some(s.replace(from.as_str(), to.as_str()))
        }
        "trim" => Some(args.first()?.trim().to_string()),
        "lower" => Some(args.first()?.to_lowercase()),
        "upper" => Some(args.first()?.to_uppercase()),
        "caps" => {
            let s = args.first()?;
            Some(capitalize_words(s))
        }

        // Logic functions
        "if" => {
            let cond = args.first()?;
            if !cond.is_empty() {
                Some(args.get(1)?.clone())
            } else {
                Some(args.get(2).cloned().unwrap_or_default())
            }
        }
        "if2" => {
            let a = args.first()?;
            if !a.is_empty() {
                Some(a.clone())
            } else {
                Some(args.get(1)?.clone())
            }
        }
        "if3" => args
            .iter()
            .find(|a| !a.is_empty())
            .cloned()
            .or(Some(String::new())),

        // Numeric functions
        "num" => {
            let n = args.first()?;
            let digits: usize = args.get(1)?.parse().ok()?;
            Some(format!("{n:0>digits$}"))
        }
        "div" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            if b == 0 {
                return Some(String::new());
            }
            Some((a / b).to_string())
        }
        "mod" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            if b == 0 {
                return Some(String::new());
            }
            Some((a % b).to_string())
        }

        // Path functions
        "directory" => {
            let p = Path::new(args.first()?);
            p.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(String::from)
        }
        "directory_path" => {
            let p = Path::new(args.first()?);
            p.parent().and_then(|p| p.to_str()).map(String::from)
        }
        "ext" => {
            let p = Path::new(args.first()?);
            p.extension().and_then(|e| e.to_str()).map(String::from)
        }
        "filename" => {
            let p = Path::new(args.first()?);
            p.file_stem().and_then(|s| s.to_str()).map(String::from)
        }

        // Meta functions
        "info" => Some(args.first()?.clone()),
        "len" => Some(args.first()?.len().to_string()),

        _ => None,
    }
}

fn capitalize_words(s: &str) -> String {
    s.split_inclusive(char::is_whitespace)
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left() {
        assert_eq!(
            call_function("left", &["hello".into(), "3".into()]),
            Some("hel".into())
        );
    }

    #[test]
    fn right() {
        assert_eq!(
            call_function("right", &["hello".into(), "3".into()]),
            Some("llo".into())
        );
    }

    #[test]
    fn pad_left() {
        assert_eq!(
            call_function("pad", &["42".into(), "5".into()]),
            Some("   42".into())
        );
    }

    #[test]
    fn pad_right() {
        assert_eq!(
            call_function("pad_right", &["42".into(), "5".into()]),
            Some("42   ".into())
        );
    }

    #[test]
    fn replace() {
        assert_eq!(
            call_function(
                "replace",
                &["hello world".into(), "world".into(), "rust".into()]
            ),
            Some("hello rust".into())
        );
    }

    #[test]
    fn trim() {
        assert_eq!(
            call_function("trim", &["  hello  ".into()]),
            Some("hello".into())
        );
    }

    #[test]
    fn lower() {
        assert_eq!(
            call_function("lower", &["HELLO".into()]),
            Some("hello".into())
        );
    }

    #[test]
    fn upper() {
        assert_eq!(
            call_function("upper", &["hello".into()]),
            Some("HELLO".into())
        );
    }

    #[test]
    fn caps() {
        assert_eq!(
            call_function("caps", &["hello world".into()]),
            Some("Hello World".into())
        );
    }

    #[test]
    fn if_nonempty() {
        assert_eq!(
            call_function("if", &["yes".into(), "true".into(), "false".into()]),
            Some("true".into())
        );
    }

    #[test]
    fn if_empty() {
        assert_eq!(
            call_function("if", &["".into(), "true".into(), "false".into()]),
            Some("false".into())
        );
    }

    #[test]
    fn if2_first() {
        assert_eq!(
            call_function("if2", &["first".into(), "second".into()]),
            Some("first".into())
        );
    }

    #[test]
    fn if2_fallback() {
        assert_eq!(
            call_function("if2", &["".into(), "second".into()]),
            Some("second".into())
        );
    }

    #[test]
    fn if3_third() {
        assert_eq!(
            call_function("if3", &["".into(), "".into(), "third".into()]),
            Some("third".into())
        );
    }

    #[test]
    fn num_zero_pad() {
        assert_eq!(
            call_function("num", &["5".into(), "3".into()]),
            Some("005".into())
        );
    }

    #[test]
    fn div() {
        assert_eq!(
            call_function("div", &["10".into(), "3".into()]),
            Some("3".into())
        );
    }

    #[test]
    fn div_by_zero() {
        assert_eq!(
            call_function("div", &["10".into(), "0".into()]),
            Some("".into())
        );
    }

    #[test]
    fn modulo() {
        assert_eq!(
            call_function("mod", &["10".into(), "3".into()]),
            Some("1".into())
        );
    }

    #[test]
    fn directory() {
        assert_eq!(
            call_function("directory", &["/music/artist/album/track.flac".into()]),
            Some("album".into())
        );
    }

    #[test]
    fn directory_path() {
        assert_eq!(
            call_function("directory_path", &["/music/artist/album/track.flac".into()]),
            Some("/music/artist/album".into())
        );
    }

    #[test]
    fn ext() {
        assert_eq!(
            call_function("ext", &["track.flac".into()]),
            Some("flac".into())
        );
    }

    #[test]
    fn filename() {
        assert_eq!(
            call_function("filename", &["track.flac".into()]),
            Some("track".into())
        );
    }

    #[test]
    fn len() {
        assert_eq!(call_function("len", &["hello".into()]), Some("5".into()));
    }

    #[test]
    fn unknown_function() {
        assert_eq!(call_function("nonexistent", &["arg".into()]), None);
    }
}
