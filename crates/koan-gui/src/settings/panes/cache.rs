//! What the download cache keeps, read from the limit as the core reads it,
//! and turning a typed limit into one.

/// How much of what is downloaded to play the cache keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keeping {
    Limited,
    Unlimited,
    /// Only what plays next.
    Off,
}

impl Keeping {
    pub const ALL: [Keeping; 3] = [Keeping::Limited, Keeping::Unlimited, Keeping::Off];

    pub fn label(self) -> &'static str {
        match self {
            Keeping::Limited => "Up to a limit",
            Keeping::Unlimited => "Without limit",
            Keeping::Off => "Only what plays next",
        }
    }

    /// No limit set is no limit, and a limit of nothing is no cache.
    pub fn of(limit: &str) -> Keeping {
        if limit.is_empty() {
            Keeping::Unlimited
        } else if koan_ffi::size_in_bytes(limit.to_owned()) == Some(0) {
            Keeping::Off
        } else {
            Keeping::Limited
        }
    }
}

const GIB: f64 = 1_073_741_824.0;

/// The limit in gigabytes, however it was written in the config, to two
/// places at most.
pub fn limit_gb(limit: &str) -> String {
    match koan_ffi::size_in_bytes(limit.to_owned()) {
        Some(bytes) if bytes > 0 => places(bytes as f64 / GIB),
        _ => String::new(),
    }
}

fn places(value: f64) -> String {
    let shown = format!("{value:.2}");
    shown.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A typed limit in gigabytes as the config writes it ("50GB"), or why it
/// is not one. `None` for nothing typed.
pub fn parse_limit(typed: &str) -> Option<Result<String, &'static str>> {
    let typed = typed.trim().replace(',', ".");
    if typed.is_empty() {
        return None;
    }
    // Two places, as the limit is written: less would round to no cache.
    match typed.parse::<f64>() {
        Ok(gb) if gb.is_finite() && gb >= 0.01 => Some(Ok(format!("{}GB", places(gb)))),
        _ => Some(Err("A limit is a number of gigabytes, such as 50.")),
    }
}

/// Whether setting `limit` deletes downloads, so it asks first: only when
/// it comes down below what is stored and below the limit set now.
pub fn lowers(limit: &str, current: &str, stored: u64) -> bool {
    let Some(bytes) = koan_ffi::size_in_bytes(limit.to_owned()) else {
        return false;
    };
    let now = match Keeping::of(current) {
        Keeping::Unlimited => None,
        _ => koan_ffi::size_in_bytes(current.to_owned()),
    };
    bytes < stored && now.is_none_or(|now| bytes < now)
}

/// Download ahead's choices: none to ten, a number set by hand past them as
/// itself, and the whole queue as `None`.
pub fn ahead_options(set: Option<u32>) -> Vec<(String, Option<u32>)> {
    let mut options = vec![
        ("Playing track only".to_owned(), Some(0)),
        ("1 track".to_owned(), Some(1)),
    ];
    options.extend((2..=10).map(|n| (format!("{n} tracks"), Some(n))));
    if let Some(n) = set.filter(|n| *n > 10) {
        options.push((format!("{n} tracks"), Some(n)));
    }
    options.push(("Whole queue".to_owned(), None));
    options
}

/// The note under the downloads section.
pub fn downloads_note(ahead: Option<u32>, keeping: Keeping) -> String {
    let mut lines = Vec::new();
    if ahead == Some(0) {
        lines.push(
            "Nothing plays gaplessly, and each track waits for its download before it starts.",
        );
    }
    match keeping {
        Keeping::Off => lines.push("Tracks are deleted once played. Downloads you ask for and favourite records stay."),
        Keeping::Limited => lines.push("Past the limit, the tracks played longest ago are deleted first. Favourite records stay."),
        Keeping::Unlimited => {}
    }
    lines.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn reads_the_limit_as_the_core_does() {
        assert_eq!(Keeping::of(""), Keeping::Unlimited);
        assert_eq!(Keeping::of("0"), Keeping::Off);
        assert_eq!(Keeping::of("50GB"), Keeping::Limited);
        assert_eq!(limit_gb("50GB"), "50");
        assert_eq!(limit_gb("512MB"), "0.5");
        assert_eq!(limit_gb(""), "");
    }

    #[test]
    fn parses_a_typed_limit() {
        assert_eq!(parse_limit("  "), None);
        assert_eq!(parse_limit("50"), Some(Ok("50GB".into())));
        assert_eq!(parse_limit("2,5"), Some(Ok("2.5GB".into())));
        assert!(parse_limit("0.001").unwrap().is_err());
        assert!(parse_limit("lots").unwrap().is_err());
    }

    #[test]
    fn asks_only_when_downloads_go() {
        let gb = 1 << 30;
        // Down below what is stored, from no limit: asks.
        assert!(lowers("10GB", "", 20 * gb));
        // Below what is stored but above the limit set now: nothing new goes.
        assert!(!lowers("10GB", "5GB", 20 * gb));
        // Above what is stored: nothing goes.
        assert!(!lowers("50GB", "", 20 * gb));
        assert!(lowers("1GB", "5GB", 20 * gb));
    }

    #[test]
    fn offers_download_ahead() {
        let options = ahead_options(Some(3));
        assert_eq!(options.first().unwrap().1, Some(0));
        assert_eq!(options.last().unwrap().1, None);
        assert_eq!(options.len(), 12);
        assert!(ahead_options(Some(25)).iter().any(|o| o.1 == Some(25)));
    }
}
