//! An instant, shown in the reader's own zone: [`Time`].
//!
//! The server knows only UTC, so the page carries the instant and the UTC reading as its fallback, and the browser
//! rewrites it in the reader's zone and language. A relative time is computed against the reader's clock, not the
//! server's, and kept current while the page is open. A counter of time spent or left is read against the server's
//! clock, which the page learns once from [`server_clock`] on its `<body>`, and steps each second.
//!
//! A `<time>` renders the same markup for the same instant every time, so a stream that sends a region only when it
//! differs sends one holding a counter only when the instant moves.
//!
//! One localiser does this for the whole page, installed by the first time to start ([`LOCALISE`]). A stream that
//! sends a region again morphs it, and the morph keeps each `<time>` but sets its text back to the server's UTC
//! reading without starting it again, so the localiser watches the document and rewrites any time whose text or
//! instant a morph changed. One timer recomputes every relative time each minute, through formatters made once, and
//! another steps the counters each second while any is moving.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use askama::Template;

use crate::attrs::{Attrs, component};

/// What every [`Time`] starts with: install the page's localiser if no time has yet, then localise this one.
///
/// The localiser reads an element's instant from `data-at` and its style from `data-time`, and learns the server's
/// clock once, as it installs, from the page's `data-kit-now`, so nothing in it comes from the server. Without one it
/// reads counters against the reader's clock. It writes only a text or title that differs
/// from what it would write, which is how its own writes, heard by its observer, come to rest. The second timer runs
/// only while a counter is moving: an elapsed one always is, a remaining one until it reaches nought.
pub(crate) const LOCALISE: &str = concat!(
    "(window.kitTime ??= (k => (",
    "k.m = new Intl.DateTimeFormat([], {dateStyle: 'medium', timeStyle: 'short'}),",
    " k.d = new Intl.DateTimeFormat([], {dateStyle: 'medium'}),",
    " k.c = new Intl.DateTimeFormat([], {timeStyle: 'medium', hourCycle: 'h23'}),",
    " k.r = new Intl.RelativeTimeFormat([], {numeric: 'auto'}),",
    " k.span = ms => (s => s < 60 ? s + 's' : (m => m < 60 ? m + 'm ' + String(s % 60).padStart(2, '0') + 's'",
    " : Math.floor(m / 60) + 'h ' + String(m % 60).padStart(2, '0') + 'm')(Math.floor(s / 60)))(Math.max(0, Math.round(ms / 1000))),",
    " k.skew = (n => n ? n - Date.now() : 0)(+document.querySelector('[data-kit-now]')?.dataset.kitNow || 0),",
    " k.now = () => Date.now() + k.skew,",
    " k.read = (t, at, s) => s == 'date' ? [k.d.format(at), ''] : s == 'clock' ? [k.c.format(at), '']",
    " : s == 'elapsed' ? [k.span(k.now(t) - at), k.m.format(at)] : s == 'remaining' ? [k.span(at - k.now(t)), k.m.format(at)]",
    " : s == 'relative' ? [(x => (u => k.r.format(Math.round(x / u[0]), u[1]))",
    "([[31536000, 'year'], [2592000, 'month'], [604800, 'week'], [86400, 'day'], [3600, 'hour']].find(u => Math.abs(x) >= u[0]) || [60, 'minute']))",
    "((at - Date.now()) / 1000), k.m.format(at)] : [k.m.format(at), ''],",
    " k.f = t => (v => (t.textContent != v[0] && (t.textContent = v[0]), t.title != v[1] && (t.title = v[1]),",
    " t.dataset.time == 'elapsed' || t.dataset.time == 'remaining' && k.now(t) < +t.dataset.at))(k.read(t, +t.dataset.at, t.dataset.time)),",
    " k.tick = () => k.ticking = [...document.querySelectorAll('time[data-time=elapsed], time[data-time=remaining]')].map(t => k.f(t))",
    ".some(Boolean) && setTimeout(k.tick, 1000 - Date.now() % 1000),",
    " k.go = t => k.f(t) && !k.ticking && (k.ticking = setTimeout(k.tick, 1000 - Date.now() % 1000)),",
    " new MutationObserver(ms => ms.forEach(x => (t => t && k.go(t))",
    "((x.target.nodeType == 1 ? x.target : x.target.parentElement)?.closest('time[data-at][data-time]'))))",
    ".observe(document.documentElement, {subtree: true, childList: true, characterData: true, attributeFilter: ['data-at', 'title']}),",
    " setInterval(() => document.querySelectorAll('time[data-time=relative]').forEach(k.f), 60000), k.go))({}))(el)",
);

/// An instant, as an `i64` since the Unix epoch, taken by value or by reference: what [`Time::millis`] and
/// [`Time::seconds`] take, so a template hands over a field or a loop's `&&i64` as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch(i64);

impl From<i64> for Epoch {
    fn from(number: i64) -> Self {
        Self(number)
    }
}

impl From<&i64> for Epoch {
    fn from(number: &i64) -> Self {
        Self(*number)
    }
}

impl From<&&i64> for Epoch {
    fn from(number: &&i64) -> Self {
        Self(**number)
    }
}

/// How a [`Time`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Moment,
    Date,
    Clock,
    Relative,
    Elapsed,
    Remaining,
}

/// An instant in a `<time>` element: `kit::Time::millis(at).relative()`.
///
/// Drawn with the UTC reading as its text and an ISO 8601 `datetime`, and rewritten by the browser, again after a
/// morph puts the UTC reading back: the date and time by default, the [`date`](Self::date) alone, the
/// [`clock`](Self::clock) to the second for a log line, or [`relative`](Self::relative) to now ("3 days ago"), which
/// the browser recomputes each minute and whose title is the date and time. A counter, [`elapsed`](Self::elapsed)
/// since the instant or [`remaining`](Self::remaining) until it, steps each second.
#[must_use = "a time draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "time.html")]
pub struct Time<'a> {
    at: i64,
    style: Style,
    now: Option<i64>,
    attrs: Attrs<'a>,
}

impl Time<'_> {
    /// The instant `at`, in milliseconds since the Unix epoch.
    pub fn millis(at: impl Into<Epoch>) -> Self {
        Self {
            at: at.into().0,
            style: Style::Moment,
            now: None,
            attrs: Attrs::default(),
        }
    }

    /// The instant `at`, in seconds since the Unix epoch.
    pub fn seconds(at: impl Into<Epoch>) -> Self {
        Self::millis(at.into().0.saturating_mul(1000))
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

    /// How long since the instant, stepping each second: the time spent on a stage, "4m 07s". Its title is the
    /// instant's date and time.
    pub fn elapsed(mut self) -> Self {
        self.style = Style::Elapsed;
        self
    }

    /// How long until the instant, stepping each second and stopping at nought: the time left before a deadline,
    /// "1h 05m". Its title is the instant's date and time.
    pub fn remaining(mut self) -> Self {
        self.style = Style::Remaining;
        self
    }

    /// When the counter's reading was taken, in milliseconds since the Unix epoch, for the words it says before the
    /// browser starts it: "4m 07s" rather than the instant's date and time. Give it only a stable value, such as when a
    /// measurement was made, never the time of rendering, or every render differs and a stream re-sends the region
    /// each time. Once started, the counter reads the page's [`server_clock`].
    pub fn as_of(mut self, now: impl Into<Epoch>) -> Self {
        self.now = Some(now.into().0);
        self
    }

    /// The style, as `data-time` names it for the localiser.
    fn kind(&self) -> &'static str {
        match self.style {
            Style::Moment => "moment",
            Style::Date => "date",
            Style::Clock => "clock",
            Style::Relative => "relative",
            Style::Elapsed => "elapsed",
            Style::Remaining => "remaining",
        }
    }

    /// What the element says before the browser rewrites it: a counter's reading when [`as_of`](Self::as_of) says
    /// when it was taken, else the instant in UTC.
    fn reading(&self) -> String {
        match (self.style, self.now) {
            (Style::Elapsed, Some(now)) => span(now.saturating_sub(self.at)),
            (Style::Remaining, Some(now)) => span(self.at.saturating_sub(now)),
            (style, _) => Utc::of(self.at).reading(style),
        }
    }

    fn utc(&self) -> Utc {
        Utc::of(self.at)
    }
}

component!(Time);

/// The attribute that tells the page the server's clock, for the `<body>` of every page holding a counter:
/// `<body{{ kit::server_clock() }}>`. Counters are read against it, so a reader whose clock is wrong still sees the
/// right time spent or left. It belongs on the page, rendered once with it, never in a region a stream sends again.
#[must_use]
pub fn server_clock() -> ServerClock {
    ServerClock(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
            }),
    )
}

/// What [`server_clock`] writes into a tag: ` data-kit-now="…"`, the server's clock in milliseconds since the Unix
/// epoch.
#[derive(Clone, Copy, Debug)]
pub struct ServerClock(i64);

impl fmt::Display for ServerClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, " data-kit-now=\"{}\"", self.0)
    }
}

impl askama::filters::HtmlSafe for ServerClock {}

/// A counter's reading: seconds under a minute, minutes and seconds under an hour, then hours and minutes. Nought
/// below nought. The localiser writes the same.
fn span(millis: i64) -> String {
    let seconds = (millis.max(0) + 500) / 1000;
    let minutes = seconds / 60;
    if seconds < 60 {
        format!("{seconds}s")
    } else if minutes < 60 {
        format!("{minutes}m {:02}s", seconds % 60)
    } else {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    }
}

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
            Style::Moment | Style::Relative | Style::Elapsed | Style::Remaining => {
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
        assert!(html.contains("data-time=\"moment\""), "{html}");
        assert!(html.contains("window.kitTime ??="), "{html}");
        let clock = Time::millis(1_795_791_315_000).clock().to_string();
        assert!(clock.ends_with(">14:55:15</time>"), "{clock}");
        assert!(clock.contains("data-time=\"clock\""), "{clock}");
    }

    #[test]
    fn a_relative_time_keeps_current() {
        let html = Time::millis(0).relative().to_string();
        assert!(html.contains("data-time=\"relative\""), "{html}");
        assert!(LOCALISE.contains("Intl.RelativeTimeFormat"));
        assert!(LOCALISE.contains("time[data-time=relative]"));
    }

    #[test]
    fn a_time_takes_its_instant_as_a_template_holds_it() {
        let at: i64 = 1_795_791_315_000;
        let field = &at;
        let looped = &field;
        assert_eq!(
            Time::millis(looped).to_string(),
            Time::millis(at).to_string()
        );
        let seconds: i64 = 1_795_791_315;
        let held = &seconds;
        assert_eq!(
            Time::seconds(held).to_string(),
            Time::millis(at).to_string()
        );
    }

    #[test]
    fn a_counter_renders_the_same_for_the_same_instant() {
        let first = Time::millis(1_000).elapsed().to_string();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(
            first,
            Time::millis(1_000).elapsed().to_string(),
            "a stream that sends only what differs must not re-send an unchanged counter"
        );
        assert!(!first.contains("data-now"), "{first}");
        assert!(
            first.ends_with(">1970-01-01 00:00 UTC</time>"),
            "without a reading taken, the fallback is the instant: {first}"
        );
        assert!(
            LOCALISE.contains("'[data-kit-now]'"),
            "the clock is the page's, learned once"
        );
        let clock = server_clock().to_string();
        assert!(clock.starts_with(" data-kit-now=\""), "{clock}");
    }

    #[test]
    fn a_counter_is_read_against_the_servers_clock() {
        let elapsed = Time::millis(1_000).elapsed().as_of(248_000).to_string();
        assert!(elapsed.contains("data-time=\"elapsed\""), "{elapsed}");
        assert!(elapsed.ends_with(">4m 07s</time>"), "{elapsed}");
        let left = Time::millis(3_901_000).remaining().as_of(1_000).to_string();
        assert!(left.ends_with(">1h 05m</time>"), "{left}");
        let past = Time::millis(0).remaining().as_of(9_000).to_string();
        assert!(
            past.ends_with(">0s</time>"),
            "a deadline passed reads nought: {past}"
        );
    }

    #[test]
    fn counters_step_together_and_only_while_moving() {
        assert_eq!(LOCALISE.matches("setTimeout(k.tick").count(), 2);
        assert!(LOCALISE.contains("time[data-time=elapsed], time[data-time=remaining]"));
        assert!(
            LOCALISE.contains(".some(Boolean) && setTimeout"),
            "the second timer stops once nothing is moving"
        );
        assert_eq!(span(59_400), "59s");
        assert_eq!(span(65_000), "1m 05s");
        assert_eq!(span(3_900_000), "1h 05m");
        assert_eq!(span(-5), "0s");
    }

    #[test]
    fn every_time_shares_one_localiser() {
        for html in [
            Time::millis(0).to_string(),
            Time::millis(0).date().to_string(),
            Time::millis(0).clock().to_string(),
            Time::millis(0).relative().to_string(),
        ] {
            assert!(
                !html.contains("on-interval"),
                "a timer per time would start hundreds on a long list: {html}"
            );
            assert_eq!(html.matches("data-init=").count(), 1, "{html}");
        }
        assert_eq!(LOCALISE.matches("setInterval").count(), 1);
        assert_eq!(LOCALISE.matches("new MutationObserver").count(), 1);
        assert!(
            LOCALISE.starts_with("(window.kitTime ??="),
            "installed once a page"
        );
        assert!(
            LOCALISE.contains("new MutationObserver"),
            "a morph sets the text back to UTC, so the localiser has to hear it"
        );
        assert!(
            !LOCALISE.contains('"'),
            "it sits in a double-quoted attribute"
        );
    }
}
