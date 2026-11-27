//! An instant, shown in the reader's own zone: [`Time`].
//!
//! The server knows only UTC, so the page carries the instant and the UTC reading as its fallback, and the browser
//! rewrites it in the reader's zone and language as the element starts. A relative time is computed against the
//! reader's clock, not the server's, and kept current while the page is open.

use askama::Template;

use crate::attrs::{Attrs, component};

/// How a [`Time`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Moment,
    Date,
    Clock,
    Relative,
}

/// An instant in a `<time>` element: `kit::Time::millis(at).relative()`.
///
/// Drawn with the UTC reading as its text and an ISO 8601 `datetime`, and rewritten by the browser as it starts: the
/// date and time by default, the [`date`](Self::date) alone, the [`clock`](Self::clock) to the second for a log line,
/// or [`relative`](Self::relative) to now ("3 days ago"), which the browser recomputes each minute and whose title is
/// the date and time.
#[must_use = "a time draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "time.html")]
pub struct Time<'a> {
    at: i64,
    style: Style,
    attrs: Attrs<'a>,
}

impl Time<'_> {
    /// The instant `at`, in milliseconds since the Unix epoch.
    pub fn millis(at: i64) -> Self {
        Self {
            at,
            style: Style::Moment,
            attrs: Attrs::default(),
        }
    }

    /// The instant `at`, in seconds since the Unix epoch.
    pub fn seconds(at: i64) -> Self {
        Self::millis(at.saturating_mul(1000))
    }

    /// The date alone.
    pub fn date(mut self) -> Self {
        self.style = Style::Date;
        self
    }

    /// The time of day to the second, on the 24-hour clock: a log line's.
    pub fn clock(mut self) -> Self {
        self.style = Style::Clock;
        self
    }

    /// How long ago, or how long until: "3 days ago", "in 2 hours".
    pub fn relative(mut self) -> Self {
        self.style = Style::Relative;
        self
    }

    fn utc(&self) -> Utc {
        Utc::of(self.at)
    }
}

component!(Time);

/// An instant's reading in UTC, worked out from the day count so the kit needs no calendar crate.
struct Utc {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
}

impl Utc {
    fn of(millis: i64) -> Self {
        let seconds = millis.div_euclid(1000);
        let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
        // Howard Hinnant's civil_from_days: the proleptic Gregorian date of a day count from 1970-01-01.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        Self {
            year: yoe + era * 400 + i64::from(month <= 2),
            month,
            day: doy - (153 * mp + 2) / 5 + 1,
            hour: rest / 3600,
            minute: rest % 3600 / 60,
            second: rest % 60,
        }
    }

    /// `2026-11-27T14:55:15Z`, for `datetime`.
    fn iso(&self) -> String {
        format!("{}T{}Z", self.date(), self.clock())
    }

    fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    fn clock(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// What the element says before the browser rewrites it.
    fn reading(&self, style: Style) -> String {
        match style {
            Style::Date => self.date(),
            Style::Clock => self.clock(),
            Style::Moment | Style::Relative => {
                format!("{} {:02}:{:02} UTC", self.date(), self.hour, self.minute)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_is_read_from_the_day_count() {
        let at = Utc::of(1_795_791_315_000);
        assert_eq!(at.iso(), "2026-11-27T14:55:15Z");
        assert_eq!(Utc::of(0).iso(), "1970-01-01T00:00:00Z");
        assert_eq!(Utc::of(951_782_400_000).date(), "2000-02-29");
        assert_eq!(Utc::of(-1).iso(), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn a_time_falls_back_to_utc_and_is_rewritten_from_its_own_data() {
        let html = Time::seconds(1_795_791_315).to_string();
        assert!(
            html.starts_with("<time datetime=\"2026-11-27T14:55:15Z\" data-at=\"1795791315000\""),
            "{html}"
        );
        assert!(html.ends_with(">2026-11-27 14:55 UTC</time>"), "{html}");
        assert!(html.contains("new Date(+el.dataset.at)"), "{html}");
        let clock = Time::millis(1_795_791_315_000).clock().to_string();
        assert!(clock.ends_with(">14:55:15</time>"), "{clock}");
        assert!(clock.contains("hourCycle: 'h23'"), "{clock}");
    }

    #[test]
    fn a_relative_time_keeps_current() {
        let html = Time::millis(0).relative().to_string();
        assert!(html.contains("Intl.RelativeTimeFormat"), "{html}");
        assert!(html.contains("data-on-interval__duration.60s="), "{html}");
        assert!(
            !Time::millis(0).to_string().contains("on-interval"),
            "only a relative time changes as the clock moves"
        );
    }
}
