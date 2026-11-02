/// A duration as a clock: `m:ss`, or `h:mm:ss` from an hour.
pub fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A size in bytes, in the largest unit that keeps it above one, as
/// Foundation's file-size style writes it (decimal units).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} {}", if n == 1 { "byte" } else { "bytes" });
    }
    let mut value = n as f64;
    let mut unit = 0;
    // Past 999.5 the next unit up, so nothing reads "1000 MB".
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let shown = if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    let shown = if shown.contains('.') {
        shown.trim_end_matches('0').trim_end_matches('.')
    } else {
        &shown
    };
    format!("{shown} {}", UNITS[unit])
}

/// How long ago Unix second `at` was, from `now`, as the Mac's short
/// relative format reads: "just now", "5 min ago", "3 hr ago", "2 days ago".
pub fn ago(at: i64, now: i64) -> String {
    let seconds = (now - at).max(0);
    match seconds {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} hr ago", s / 3600),
        s if s < 2 * 86_400 => "yesterday".into(),
        s => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, clock};

    #[test]
    fn reads_as_time_ago() {
        assert_eq!(ago(100, 110), "just now");
        assert_eq!(ago(0, 300), "5 min ago");
        assert_eq!(ago(0, 3 * 3600 + 5), "3 hr ago");
        assert_eq!(ago(0, 86_400 + 1), "yesterday");
        assert_eq!(ago(0, 5 * 86_400), "5 days ago");
        assert_eq!(ago(10, 0), "just now");
    }

    #[test]
    fn reads_as_a_clock() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(61_999), "1:01");
        assert_eq!(clock(3_600_000 + 5_000), "1:00:05");
    }
}
