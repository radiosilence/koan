use std::path::Path;

/// fb2k boolean: truthy = "1", falsy = ""
fn bool_str(v: bool) -> String {
    if v { "1".into() } else { String::new() }
}

pub fn call_function(name: &str, args: &[String]) -> Option<String> {
    match name {
        // --- String functions ---
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
        "padcut" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            let padded = format!("{s:>n$}");
            Some(padded.chars().take(n).collect())
        }
        "padcut_right" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            let padded = format!("{s:<n$}");
            Some(padded.chars().take(n).collect())
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
        "caps2" => {
            let s = args.first()?;
            Some(capitalize_words_smart(s))
        }
        "abbr" => {
            let s = args.first()?;
            Some(abbreviate(s))
        }
        "substr" => {
            let s = args.first()?;
            let from: usize = args.get(1)?.parse().ok()?;
            let to: usize = args.get(2)?.parse().ok()?;
            let chars: Vec<char> = s.chars().collect();
            let start = from.min(chars.len());
            let end = to.min(chars.len());
            if start > end {
                Some(String::new())
            } else {
                Some(chars[start..end].iter().collect())
            }
        }
        "insert" => {
            let s = args.first()?;
            let sub = args.get(1)?;
            let pos: usize = args.get(2)?.parse().ok()?;
            let mut chars: Vec<char> = s.chars().collect();
            let idx = pos.min(chars.len());
            for (i, c) in sub.chars().enumerate() {
                chars.insert(idx + i, c);
            }
            Some(chars.into_iter().collect())
        }
        "repeat" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            Some(s.repeat(n))
        }
        "stripprefix" => {
            let s = args.first()?;
            // Custom prefix list or default articles
            let prefixes = if args.len() > 1 {
                args[1..].to_vec()
            } else {
                vec!["A ".into(), "The ".into()]
            };
            for prefix in &prefixes {
                if let Some(rest) = s.strip_prefix(prefix.as_str()) {
                    return Some(rest.to_string());
                }
            }
            Some(s.clone())
        }
        "swapprefix" => {
            let s = args.first()?;
            let prefixes = if args.len() > 1 {
                args[1..].to_vec()
            } else {
                vec!["A ".into(), "The ".into()]
            };
            for prefix in &prefixes {
                if let Some(rest) = s.strip_prefix(prefix.as_str()) {
                    return Some(format!("{}, {}", rest, prefix.trim()));
                }
            }
            Some(s.clone())
        }
        "rot13" => {
            let s = args.first()?;
            Some(
                s.chars()
                    .map(|c| match c {
                        'a'..='m' | 'A'..='M' => (c as u8 + 13) as char,
                        'n'..='z' | 'N'..='Z' => (c as u8 - 13) as char,
                        _ => c,
                    })
                    .collect(),
            )
        }
        "fix_eol" => {
            let s = args.first()?;
            let replacement = args.get(1).map(|s| s.as_str()).unwrap_or(" ");
            Some(s.replace(['\r', '\n'], replacement))
        }

        // --- String search ---
        "strchr" => {
            let s = args.first()?;
            let c = args.get(1)?.chars().next()?;
            Some(s.find(c).map_or(String::new(), |i| (i + 1).to_string()))
        }
        "strrchr" => {
            let s = args.first()?;
            let c = args.get(1)?.chars().next()?;
            Some(s.rfind(c).map_or(String::new(), |i| (i + 1).to_string()))
        }
        "strstr" => {
            let s = args.first()?;
            let sub = args.get(1)?;
            Some(
                s.find(sub.as_str())
                    .map_or(String::new(), |i| (i + 1).to_string()),
            )
        }

        // --- String comparison (boolean) ---
        "strcmp" => {
            let a = args.first()?;
            let b = args.get(1)?;
            Some(bool_str(a == b))
        }
        "stricmp" => {
            let a = args.first()?;
            let b = args.get(1)?;
            Some(bool_str(a.eq_ignore_ascii_case(b)))
        }
        "longer" => {
            let a = args.first()?;
            let b = args.get(1)?;
            Some(bool_str(a.len() > b.len()))
        }
        "longest" => args.iter().max_by_key(|s| s.len()).cloned(),
        "shortest" => args.iter().min_by_key(|s| s.len()).cloned(),

        // --- Logic functions ---
        "if" => {
            let cond = args.first()?;
            if !cond.is_empty() {
                // then branch — may be empty string (valid for $if(cond,,else) pattern)
                Some(args.get(1).cloned().unwrap_or_default())
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
        "ifequal" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            if a == b {
                Some(args.get(2).cloned().unwrap_or_default())
            } else {
                Some(args.get(3).cloned().unwrap_or_default())
            }
        }
        "ifgreater" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            if a > b {
                Some(args.get(2).cloned().unwrap_or_default())
            } else {
                Some(args.get(3).cloned().unwrap_or_default())
            }
        }
        "iflonger" => {
            let s = args.first()?;
            let n: usize = args.get(1)?.parse().ok()?;
            if s.len() > n {
                Some(args.get(2).cloned().unwrap_or_default())
            } else {
                Some(args.get(3).cloned().unwrap_or_default())
            }
        }
        "select" => {
            let n: usize = args.first()?.parse().ok()?;
            if n == 0 || n > args.len() - 1 {
                Some(String::new())
            } else {
                Some(args[n].clone())
            }
        }
        "not" => {
            let a = args.first()?;
            Some(bool_str(a.is_empty()))
        }
        "and" => {
            let a = args.first()?;
            let b = args.get(1)?;
            Some(bool_str(!a.is_empty() && !b.is_empty()))
        }
        "or" => {
            let a = args.first()?;
            let b = args.get(1)?;
            Some(bool_str(!a.is_empty() || !b.is_empty()))
        }
        "xor" => {
            let a = args.first()?;
            let b = args.get(1)?;
            Some(bool_str(a.is_empty() != b.is_empty()))
        }
        "greater" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            Some(bool_str(a > b))
        }

        // --- Numeric functions ---
        "num" => {
            let n = args.first()?;
            let digits: usize = args.get(1)?.parse().ok()?;
            Some(format!("{n:0>digits$}"))
        }
        "add" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            Some((a + b).to_string())
        }
        "sub" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            Some((a - b).to_string())
        }
        "mul" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            Some((a * b).to_string())
        }
        "muldiv" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            let c: i64 = args.get(2)?.parse().ok()?;
            if c == 0 {
                return Some(String::new());
            }
            Some(((a as i128 * b as i128) / c as i128).to_string())
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
        "max" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            Some(a.max(b).to_string())
        }
        "min" => {
            let a: i64 = args.first()?.parse().ok()?;
            let b: i64 = args.get(1)?.parse().ok()?;
            Some(a.min(b).to_string())
        }
        "hex" => {
            let n: i64 = args.first()?.parse().ok()?;
            let digits = args
                .get(1)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            if digits > 0 {
                Some(format!("{n:0>width$X}", width = digits))
            } else {
                Some(format!("{n:X}"))
            }
        }

        // --- Path functions ---
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

        // --- Special character functions ---
        "tab" => {
            let n = args
                .first()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            Some("\t".repeat(n))
        }
        "crlf" => Some("\r\n".into()),
        "char" => {
            let n: u32 = args.first()?.parse().ok()?;
            char::from_u32(n).map(|c| c.to_string())
        }

        // --- Meta functions ---
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

/// Like capitalize_words but keeps articles/prepositions lowercase (unless first word).
fn capitalize_words_smart(s: &str) -> String {
    const LOWER_WORDS: &[&str] = &[
        "a", "an", "the", "and", "or", "nor", "but", "in", "on", "at", "to", "for", "of", "with",
        "by", "from", "as", "is", "vs",
    ];
    let words: Vec<&str> = s.split_inclusive(char::is_whitespace).collect();
    words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            let trimmed = word.trim().to_lowercase();
            if i > 0 && LOWER_WORDS.contains(&trimmed.as_str()) {
                word.to_lowercase()
            } else {
                let mut chars = word.chars();
                match chars.next() {
                    Some(c) => {
                        let upper: String = c.to_uppercase().collect();
                        upper + &chars.as_str().to_lowercase()
                    }
                    None => String::new(),
                }
            }
        })
        .collect()
}

/// Abbreviate: take first letter of each word.
fn abbreviate(s: &str) -> String {
    s.split_whitespace()
        .filter_map(|w| w.chars().next())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- String functions ----

    #[test]
    fn left() {
        assert_eq!(
            call_function("left", &s(&["hello", "3"])),
            Some("hel".into())
        );
    }

    #[test]
    fn left_exceeds_length() {
        assert_eq!(call_function("left", &s(&["hi", "10"])), Some("hi".into()));
    }

    #[test]
    fn left_zero() {
        assert_eq!(call_function("left", &s(&["hello", "0"])), Some("".into()));
    }

    #[test]
    fn left_unicode() {
        assert_eq!(
            call_function("left", &s(&["\u{00e9}l\u{00e8}ve", "3"])),
            Some("\u{00e9}l\u{00e8}".into())
        );
    }

    #[test]
    fn right() {
        assert_eq!(
            call_function("right", &s(&["hello", "3"])),
            Some("llo".into())
        );
    }

    #[test]
    fn right_exceeds_length() {
        assert_eq!(call_function("right", &s(&["hi", "10"])), Some("hi".into()));
    }

    #[test]
    fn pad_left() {
        assert_eq!(call_function("pad", &s(&["42", "5"])), Some("   42".into()));
    }

    #[test]
    fn pad_left_already_wider() {
        assert_eq!(
            call_function("pad", &s(&["hello", "3"])),
            Some("hello".into())
        );
    }

    #[test]
    fn pad_right() {
        assert_eq!(
            call_function("pad_right", &s(&["42", "5"])),
            Some("42   ".into())
        );
    }

    #[test]
    fn padcut() {
        assert_eq!(
            call_function("padcut", &s(&["hi", "5"])),
            Some("   hi".into())
        );
        assert_eq!(
            call_function("padcut", &s(&["hello world", "5"])),
            Some("hello".into())
        );
    }

    #[test]
    fn padcut_right() {
        assert_eq!(
            call_function("padcut_right", &s(&["hi", "5"])),
            Some("hi   ".into())
        );
        assert_eq!(
            call_function("padcut_right", &s(&["hello world", "5"])),
            Some("hello".into())
        );
    }

    #[test]
    fn replace() {
        assert_eq!(
            call_function("replace", &s(&["hello world", "world", "rust"])),
            Some("hello rust".into())
        );
    }

    #[test]
    fn replace_no_match() {
        assert_eq!(
            call_function("replace", &s(&["hello", "xyz", "abc"])),
            Some("hello".into())
        );
    }

    #[test]
    fn trim() {
        assert_eq!(
            call_function("trim", &s(&["  hello  "])),
            Some("hello".into())
        );
    }

    #[test]
    fn trim_empty() {
        assert_eq!(call_function("trim", &s(&["   "])), Some("".into()));
    }

    #[test]
    fn lower() {
        assert_eq!(call_function("lower", &s(&["HELLO"])), Some("hello".into()));
    }

    #[test]
    fn upper() {
        assert_eq!(call_function("upper", &s(&["hello"])), Some("HELLO".into()));
    }

    #[test]
    fn caps() {
        assert_eq!(
            call_function("caps", &s(&["hello world"])),
            Some("Hello World".into())
        );
    }

    #[test]
    fn caps_mixed_case() {
        assert_eq!(
            call_function("caps", &s(&["hELLO wORLD"])),
            Some("Hello World".into())
        );
    }

    #[test]
    fn caps2_articles() {
        assert_eq!(
            call_function("caps2", &s(&["the quick and the dead"])),
            Some("The Quick and the Dead".into())
        );
    }

    #[test]
    fn abbr() {
        assert_eq!(
            call_function("abbr", &s(&["Aphex Twin"])),
            Some("AT".into())
        );
        assert_eq!(
            call_function("abbr", &s(&["The Chemical Brothers"])),
            Some("TCB".into())
        );
    }

    #[test]
    fn substr() {
        assert_eq!(
            call_function("substr", &s(&["hello world", "6", "11"])),
            Some("world".into())
        );
    }

    #[test]
    fn substr_out_of_bounds() {
        assert_eq!(
            call_function("substr", &s(&["hi", "0", "100"])),
            Some("hi".into())
        );
    }

    #[test]
    fn insert() {
        assert_eq!(
            call_function("insert", &s(&["hello", "XX", "2"])),
            Some("heXXllo".into())
        );
    }

    #[test]
    fn repeat() {
        assert_eq!(
            call_function("repeat", &s(&["ab", "3"])),
            Some("ababab".into())
        );
    }

    #[test]
    fn repeat_zero() {
        assert_eq!(call_function("repeat", &s(&["ab", "0"])), Some("".into()));
    }

    #[test]
    fn stripprefix_the() {
        assert_eq!(
            call_function("stripprefix", &s(&["The Beatles"])),
            Some("Beatles".into())
        );
    }

    #[test]
    fn stripprefix_a() {
        assert_eq!(
            call_function("stripprefix", &s(&["A Perfect Circle"])),
            Some("Perfect Circle".into())
        );
    }

    #[test]
    fn stripprefix_no_match() {
        assert_eq!(
            call_function("stripprefix", &s(&["Radiohead"])),
            Some("Radiohead".into())
        );
    }

    #[test]
    fn swapprefix() {
        assert_eq!(
            call_function("swapprefix", &s(&["The Beatles"])),
            Some("Beatles, The".into())
        );
    }

    #[test]
    fn swapprefix_no_match() {
        assert_eq!(
            call_function("swapprefix", &s(&["Radiohead"])),
            Some("Radiohead".into())
        );
    }

    #[test]
    fn rot13() {
        assert_eq!(call_function("rot13", &s(&["Hello"])), Some("Uryyb".into()));
        assert_eq!(call_function("rot13", &s(&["Uryyb"])), Some("Hello".into()));
    }

    #[test]
    fn fix_eol() {
        assert_eq!(
            call_function("fix_eol", &s(&["line1\nline2\rline3"])),
            Some("line1 line2 line3".into())
        );
    }

    #[test]
    fn fix_eol_custom() {
        assert_eq!(
            call_function("fix_eol", &s(&["a\nb", " | "])),
            Some("a | b".into())
        );
    }

    // ---- String search ----

    #[test]
    fn strchr_found() {
        assert_eq!(
            call_function("strchr", &s(&["hello", "l"])),
            Some("3".into())
        );
    }

    #[test]
    fn strchr_not_found() {
        assert_eq!(
            call_function("strchr", &s(&["hello", "z"])),
            Some("".into())
        );
    }

    #[test]
    fn strrchr_found() {
        assert_eq!(
            call_function("strrchr", &s(&["hello", "l"])),
            Some("4".into())
        );
    }

    #[test]
    fn strstr_found() {
        assert_eq!(
            call_function("strstr", &s(&["hello world", "world"])),
            Some("7".into())
        );
    }

    #[test]
    fn strstr_not_found() {
        assert_eq!(
            call_function("strstr", &s(&["hello", "xyz"])),
            Some("".into())
        );
    }

    // ---- String comparison (boolean) ----

    #[test]
    fn strcmp_equal() {
        assert_eq!(
            call_function("strcmp", &s(&["hello", "hello"])),
            Some("1".into())
        );
    }

    #[test]
    fn strcmp_not_equal() {
        assert_eq!(
            call_function("strcmp", &s(&["hello", "HELLO"])),
            Some("".into())
        );
    }

    #[test]
    fn stricmp_equal() {
        assert_eq!(
            call_function("stricmp", &s(&["hello", "HELLO"])),
            Some("1".into())
        );
    }

    #[test]
    fn stricmp_not_equal() {
        assert_eq!(
            call_function("stricmp", &s(&["hello", "world"])),
            Some("".into())
        );
    }

    #[test]
    fn stricmp_various_artists() {
        assert_eq!(
            call_function("stricmp", &s(&["Various Artists", "Various Artists"])),
            Some("1".into())
        );
        assert_eq!(
            call_function("stricmp", &s(&["various artists", "Various Artists"])),
            Some("1".into())
        );
        assert_eq!(
            call_function("stricmp", &s(&["Aphex Twin", "Various Artists"])),
            Some("".into())
        );
    }

    #[test]
    fn longer() {
        assert_eq!(
            call_function("longer", &s(&["hello", "hi"])),
            Some("1".into())
        );
        assert_eq!(
            call_function("longer", &s(&["hi", "hello"])),
            Some("".into())
        );
        assert_eq!(call_function("longer", &s(&["hi", "hi"])), Some("".into()));
    }

    #[test]
    fn longest() {
        assert_eq!(
            call_function("longest", &s(&["a", "hello", "hi"])),
            Some("hello".into())
        );
    }

    #[test]
    fn shortest() {
        assert_eq!(
            call_function("shortest", &s(&["hello", "a", "hi"])),
            Some("a".into())
        );
    }

    // ---- Logic functions ----

    #[test]
    fn if_nonempty() {
        assert_eq!(
            call_function("if", &s(&["yes", "true", "false"])),
            Some("true".into())
        );
    }

    #[test]
    fn if_empty() {
        assert_eq!(
            call_function("if", &s(&["", "true", "false"])),
            Some("false".into())
        );
    }

    #[test]
    fn if_empty_then_branch() {
        // $if(cond,,else) — empty then branch is valid
        assert_eq!(
            call_function("if", &s(&["yes", "", "fallback"])),
            Some("".into())
        );
    }

    #[test]
    fn if_no_else() {
        // $if(cond,then) — missing else defaults to ""
        assert_eq!(call_function("if", &s(&["", "true"])), Some("".into()));
    }

    #[test]
    fn if2_first() {
        assert_eq!(
            call_function("if2", &s(&["first", "second"])),
            Some("first".into())
        );
    }

    #[test]
    fn if2_fallback() {
        assert_eq!(
            call_function("if2", &s(&["", "second"])),
            Some("second".into())
        );
    }

    #[test]
    fn if3_third() {
        assert_eq!(
            call_function("if3", &s(&["", "", "third"])),
            Some("third".into())
        );
    }

    #[test]
    fn if3_all_empty() {
        assert_eq!(call_function("if3", &s(&["", "", ""])), Some("".into()));
    }

    #[test]
    fn ifequal_match() {
        assert_eq!(
            call_function("ifequal", &s(&["5", "5", "yes", "no"])),
            Some("yes".into())
        );
    }

    #[test]
    fn ifequal_no_match() {
        assert_eq!(
            call_function("ifequal", &s(&["5", "3", "yes", "no"])),
            Some("no".into())
        );
    }

    #[test]
    fn ifgreater_true() {
        assert_eq!(
            call_function("ifgreater", &s(&["10", "5", "yes", "no"])),
            Some("yes".into())
        );
    }

    #[test]
    fn ifgreater_false() {
        assert_eq!(
            call_function("ifgreater", &s(&["3", "5", "yes", "no"])),
            Some("no".into())
        );
    }

    #[test]
    fn iflonger_true() {
        assert_eq!(
            call_function("iflonger", &s(&["hello", "3", "yes", "no"])),
            Some("yes".into())
        );
    }

    #[test]
    fn iflonger_false() {
        assert_eq!(
            call_function("iflonger", &s(&["hi", "5", "yes", "no"])),
            Some("no".into())
        );
    }

    #[test]
    fn select_valid() {
        assert_eq!(
            call_function("select", &s(&["2", "a", "b", "c"])),
            Some("b".into())
        );
    }

    #[test]
    fn select_out_of_range() {
        assert_eq!(
            call_function("select", &s(&["0", "a", "b"])),
            Some("".into())
        );
        assert_eq!(
            call_function("select", &s(&["99", "a", "b"])),
            Some("".into())
        );
    }

    #[test]
    fn not_truthy() {
        assert_eq!(call_function("not", &s(&["hello"])), Some("".into()));
    }

    #[test]
    fn not_falsy() {
        assert_eq!(call_function("not", &s(&[""])), Some("1".into()));
    }

    #[test]
    fn and_both_truthy() {
        assert_eq!(call_function("and", &s(&["a", "b"])), Some("1".into()));
    }

    #[test]
    fn and_one_empty() {
        assert_eq!(call_function("and", &s(&["a", ""])), Some("".into()));
    }

    #[test]
    fn or_one_truthy() {
        assert_eq!(call_function("or", &s(&["", "b"])), Some("1".into()));
    }

    #[test]
    fn or_both_empty() {
        assert_eq!(call_function("or", &s(&["", ""])), Some("".into()));
    }

    #[test]
    fn xor_different() {
        assert_eq!(call_function("xor", &s(&["a", ""])), Some("1".into()));
    }

    #[test]
    fn xor_same() {
        assert_eq!(call_function("xor", &s(&["a", "b"])), Some("".into()));
        assert_eq!(call_function("xor", &s(&["", ""])), Some("".into()));
    }

    #[test]
    fn greater_true() {
        assert_eq!(call_function("greater", &s(&["10", "5"])), Some("1".into()));
    }

    #[test]
    fn greater_false() {
        assert_eq!(call_function("greater", &s(&["3", "5"])), Some("".into()));
    }

    #[test]
    fn greater_equal() {
        assert_eq!(call_function("greater", &s(&["5", "5"])), Some("".into()));
    }

    // ---- Numeric functions ----

    #[test]
    fn num_zero_pad() {
        assert_eq!(call_function("num", &s(&["5", "3"])), Some("005".into()));
    }

    #[test]
    fn num_already_wide() {
        assert_eq!(
            call_function("num", &s(&["12345", "3"])),
            Some("12345".into())
        );
    }

    #[test]
    fn add() {
        assert_eq!(call_function("add", &s(&["3", "4"])), Some("7".into()));
    }

    #[test]
    fn add_negative() {
        assert_eq!(call_function("add", &s(&["10", "-3"])), Some("7".into()));
    }

    #[test]
    fn sub() {
        assert_eq!(call_function("sub", &s(&["10", "3"])), Some("7".into()));
    }

    #[test]
    fn mul() {
        assert_eq!(call_function("mul", &s(&["3", "4"])), Some("12".into()));
    }

    #[test]
    fn muldiv() {
        assert_eq!(
            call_function("muldiv", &s(&["10", "3", "2"])),
            Some("15".into())
        );
    }

    #[test]
    fn muldiv_by_zero() {
        assert_eq!(
            call_function("muldiv", &s(&["10", "3", "0"])),
            Some("".into())
        );
    }

    #[test]
    fn div() {
        assert_eq!(call_function("div", &s(&["10", "3"])), Some("3".into()));
    }

    #[test]
    fn div_by_zero() {
        assert_eq!(call_function("div", &s(&["10", "0"])), Some("".into()));
    }

    #[test]
    fn modulo() {
        assert_eq!(call_function("mod", &s(&["10", "3"])), Some("1".into()));
    }

    #[test]
    fn mod_by_zero() {
        assert_eq!(call_function("mod", &s(&["10", "0"])), Some("".into()));
    }

    #[test]
    fn max() {
        assert_eq!(call_function("max", &s(&["3", "7"])), Some("7".into()));
    }

    #[test]
    fn min() {
        assert_eq!(call_function("min", &s(&["3", "7"])), Some("3".into()));
    }

    #[test]
    fn hex_basic() {
        assert_eq!(call_function("hex", &s(&["255"])), Some("FF".into()));
    }

    #[test]
    fn hex_padded() {
        assert_eq!(call_function("hex", &s(&["255", "4"])), Some("00FF".into()));
    }

    // ---- Path functions ----

    #[test]
    fn directory() {
        assert_eq!(
            call_function("directory", &s(&["/music/artist/album/track.flac"])),
            Some("album".into())
        );
    }

    #[test]
    fn directory_path() {
        assert_eq!(
            call_function("directory_path", &s(&["/music/artist/album/track.flac"])),
            Some("/music/artist/album".into())
        );
    }

    #[test]
    fn ext() {
        assert_eq!(
            call_function("ext", &s(&["track.flac"])),
            Some("flac".into())
        );
    }

    #[test]
    fn ext_no_extension() {
        assert_eq!(call_function("ext", &s(&["track"])), None);
    }

    #[test]
    fn filename() {
        assert_eq!(
            call_function("filename", &s(&["track.flac"])),
            Some("track".into())
        );
    }

    #[test]
    fn filename_with_path() {
        assert_eq!(
            call_function("filename", &s(&["/music/track.flac"])),
            Some("track".into())
        );
    }

    // ---- Special characters ----

    #[test]
    fn tab() {
        assert_eq!(call_function("tab", &s(&[])), Some("\t".into()));
        assert_eq!(call_function("tab", &s(&["3"])), Some("\t\t\t".into()));
    }

    #[test]
    fn crlf() {
        assert_eq!(call_function("crlf", &s(&[])), Some("\r\n".into()));
    }

    #[test]
    fn char_function() {
        assert_eq!(call_function("char", &s(&["65"])), Some("A".into()));
        assert_eq!(
            call_function("char", &s(&["8226"])),
            Some("\u{2022}".into())
        );
    }

    // ---- Meta ----

    #[test]
    fn info() {
        assert_eq!(call_function("info", &s(&["test"])), Some("test".into()));
    }

    #[test]
    fn len() {
        assert_eq!(call_function("len", &s(&["hello"])), Some("5".into()));
    }

    #[test]
    fn len_empty() {
        assert_eq!(call_function("len", &s(&[""])), Some("0".into()));
    }

    #[test]
    fn unknown_function() {
        assert_eq!(call_function("nonexistent", &s(&["arg"])), None);
    }

    // ---- Missing args return None ----

    #[test]
    fn left_no_args() {
        assert_eq!(call_function("left", &s(&[])), None);
    }

    #[test]
    fn div_non_numeric() {
        assert_eq!(call_function("div", &s(&["abc", "3"])), None);
    }

    // Helper to make arg arrays less noisy
    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }
}
