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

/// A size as people read one: "1.2 GB", in decimal units as the Mac shows
/// them.
pub fn bytes(n: u64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e9 => format!("{:.1} GB", n / 1e9),
        n if n >= 1e6 => format!("{:.0} MB", n / 1e6),
        n if n >= 1e3 => format!("{:.0} KB", n / 1e3),
        n => format!("{n:.0} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::clock;

    #[test]
    fn reads_as_a_clock() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(61_999), "1:01");
        assert_eq!(clock(3_600_000 + 5_000), "1:00:05");
    }
}
