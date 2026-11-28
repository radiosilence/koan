//! Bytes on their way, moving between the server's readings: [`Transfer`].
//!
//! A server reads a transfer now and then: bytes done, the total and the rate. The page draws each reading as a
//! [`Meter`] and a line of bytes, rate and time left, and the page's one mover ([`MOVE`]) advances both from the rate
//! four times a second until the next reading replaces it. It stops at the total, at a rate of nought, and two report
//! intervals after the reading arrived, so a transfer that has stalled without saying so stops moving rather than
//! running on.
//!
//! A reading is timed from when the page received it, not from when the server measured it, so a reader whose clock
//! is wrong still sees it move at its rate. The measurement's instant tells one reading from the next. The same reading
//! renders the same markup every time, so a stream that sends a region only when it differs sends one holding a
//! transfer only when the reading changes.

use std::time::Duration;

use askama::Template;

use crate::arg::Text;
use crate::attrs::{Attrs, component};
use crate::status::{Meter, Tone};
use crate::time::{Epoch, span};

/// How often a transfer is read, unless [`Transfer::every`] says otherwise.
const REPORT: Duration = Duration::from_secs(1);

/// What every [`Transfer`] starts with: install the page's mover if no transfer has yet, then move this one.
///
/// The mover reads a transfer's reading from its `data-bytes`, `data-total`, `data-rate` and `data-at`, and the report
/// interval from `data-every`. It keeps when each reading arrived on the element itself, which a morph keeps, and
/// takes a reading as new when its bytes or instant change. One timer redraws every moving transfer and stops once
/// none is moving or the page is hidden; it starts again when the page shows or a reading arrives. It writes only a
/// value or text that differs from what it would write.
pub(crate) const MOVE: &str = concat!(
    "(window.kitTransfer ??= (k => (",
    "k.size = b => (u => u ? (Math.round(b / 1024 ** u * 10) / 10).toFixed(1) + ' ' + ['B', 'KB', 'MB', 'GB', 'TB'][u]",
    " : Math.floor(b) + ' B')([1, 2, 3, 4].filter(u => b >= 1024 ** u).length),",
    " k.span = ms => (s => s < 60 ? s + 's' : (m => m < 60 ? m + 'm ' + String(s % 60).padStart(2, '0') + 's'",
    " : Math.floor(m / 60) + 'h ' + String(m % 60).padStart(2, '0') + 'm')(Math.floor(s / 60)))(Math.max(0, Math.round(ms / 1000))),",
    " k.put = (e, v) => e && e.textContent != v && (e.textContent = v),",
    " k.f = t => (d => (b => (n => (r => (h => (seen => (s => (x => (",
    "(p => p && n && p.value != x && (p.value = x))(t.querySelector('progress')),",
    " k.put(t.querySelector('[data-kit-done]'), k.size(x)),",
    " k.put(t.querySelector('[data-kit-left]'), r > 0 && n && x < n ? k.span((n - x) / r * 1000) + ' left' : ''),",
    " r > 0 && s < h && !(n && x >= n)))",
    "(r > 0 ? (v => n ? Math.min(n, v) : v)(b + r * Math.min(s, h) / 1000) : b))",
    "(performance.now() - seen.t))",
    "(t._kitseen?.key == d.bytes + ' ' + d.at ? t._kitseen : (t._kitseen = {key: d.bytes + ' ' + d.at, t: performance.now()})))",
    "(2 * (+d.every || 1000)))(+d.rate))(+d.total))(+d.bytes))(t.dataset),",
    " k.tick = () => k.ticking = !document.hidden && [...document.querySelectorAll('[data-kit-transfer]')].map(k.f)",
    ".some(Boolean) && setTimeout(k.tick, 250),",
    " k.go = t => k.f(t) && !k.ticking && !document.hidden && (k.ticking = setTimeout(k.tick, 250)),",
    " new MutationObserver(ms => ms.forEach(x => (t => t && k.go(t))",
    "((x.target.nodeType == 1 ? x.target : x.target.parentElement)?.closest('[data-kit-transfer]'))))",
    ".observe(document.documentElement, {subtree: true, characterData: true,",
    " attributeFilter: ['data-bytes', 'data-total', 'data-rate', 'data-at']}),",
    " document.addEventListener('visibilitychange', () => !k.ticking && k.tick()), k.go))({}))(el)",
);

/// A count of bytes, taken by value or by reference, so a template hands over a field or a loop's `&&u64` as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bytes(u64);

impl From<u64> for Bytes {
    fn from(number: u64) -> Self {
        Self(number)
    }
}

impl From<&u64> for Bytes {
    fn from(number: &u64) -> Self {
        Self(*number)
    }
}

impl From<&&u64> for Bytes {
    fn from(number: &&u64) -> Self {
        Self(**number)
    }
}

/// A size as the kit writes it, in steps of 1024 to one decimal place: `512 B`, `12.3 MB`. The same words a
/// [`Transfer`] writes, for the sizes around one.
#[must_use]
pub fn size(bytes: impl Into<Bytes>) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let bytes = bytes.into().0;
    let (mut unit, mut step) = (0, 1024_u64);
    while unit < UNITS.len() - 1 && bytes >= step {
        unit += 1;
        step = step.saturating_mul(1024);
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let scaled = bytes as f64 / (step / 1024) as f64;
    format!("{:.1} {}", (scaled * 10.0).round() / 10.0, UNITS[unit])
}

/// Bytes on their way, drawn as a meter and a line of bytes done, rate and time left that move between readings:
/// `kit::Transfer::new(done, total).rate(bytes_per_second, measured_at)`.
#[must_use = "a transfer draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "transfer.html")]
pub struct Transfer<'a> {
    bytes: u64,
    total: u64,
    rate: u64,
    at: i64,
    every: Option<u128>,
    tone: Option<Tone>,
    label: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Transfer<'a> {
    /// `bytes` of `total` done, still. A total of nought is one not yet known: the meter waits and nothing is left to
    /// count down.
    pub fn new(bytes: impl Into<Bytes>, total: impl Into<Bytes>) -> Self {
        Self {
            bytes: bytes.into().0,
            total: total.into().0,
            rate: 0,
            at: 0,
            every: None,
            tone: None,
            label: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// Moving at `bytes_per_second`, as measured at `measured_at` (milliseconds since the Unix epoch). The instant
    /// marks the reading as a new one; it must be when the reading was taken, never the render time, or every render
    /// would differ. A rate of nought stands still.
    pub fn rate(
        mut self,
        bytes_per_second: impl Into<Bytes>,
        measured_at: impl Into<Epoch>,
    ) -> Self {
        self.rate = bytes_per_second.into().0;
        self.at = if self.rate > 0 {
            measured_at.into().get()
        } else {
            0
        };
        self
    }

    /// How often the server reads it, when not once a second: a transfer moves for two of these past its reading.
    pub fn every(mut self, interval: Duration) -> Self {
        self.every = (interval != REPORT).then_some(interval.as_millis());
        self
    }

    /// The meter's done part in a status's tone rather than the accent.
    pub fn tone(mut self, tone: impl Into<Option<Tone>>) -> Self {
        self.tone = tone.into();
        self
    }

    /// What it carries, for a screen reader: `"download"`.
    pub fn label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.label = label.into();
        self
    }

    fn meter(&self) -> Meter<'a> {
        let meter = if self.total > 0 {
            Meter::new(self.bytes.min(self.total), self.total)
        } else {
            Meter::waiting()
        };
        let meter = meter.label(self.label.clone());
        match self.tone {
            Some(tone) => meter.tone(tone),
            None => meter,
        }
    }

    fn moving(&self) -> bool {
        self.rate > 0
    }

    fn counts_down(&self) -> bool {
        self.moving() && self.total > 0
    }

    fn left(&self) -> String {
        if self.bytes >= self.total {
            return String::new();
        }
        let millis = u128::from(self.total - self.bytes) * 1000 / u128::from(self.rate);
        format!("{} left", span(i64::try_from(millis).unwrap_or(i64::MAX)))
    }
}

component!(Transfer);

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_790_000_000_000;

    /// The markup without the mover, whose own text names the parts it writes.
    fn markup(transfer: &Transfer<'_>) -> String {
        let html = transfer.to_string();
        let (head, rest) = html
            .split_once(" data-init=\"")
            .expect("a transfer starts its mover");
        format!(
            "{head}{}",
            &rest[rest.find('"').expect("the attribute closes") + 1..]
        )
    }

    #[test]
    fn a_size_steps_in_1024s_to_one_place() {
        assert_eq!(size(0_u64), "0 B");
        assert_eq!(size(1023_u64), "1023 B");
        assert_eq!(size(1024_u64), "1.0 KB");
        assert_eq!(size(12_900_000_u64), "12.3 MB");
        assert_eq!(size(3 * 1024_u64.pow(3)), "3.0 GB");
        assert_eq!(size(2000 * 1024_u64.pow(4)), "2000.0 TB");
    }

    #[test]
    fn a_moving_transfer_carries_its_reading_and_says_what_is_left() {
        let html = Transfer::new(10 * 1024 * 1024_u64, 40 * 1024 * 1024_u64)
            .rate(1024 * 1024_u64, AT)
            .label("download")
            .to_string();
        assert!(
            html.starts_with("<div class=\"transfer\" data-kit-transfer"),
            "{html}"
        );
        for attr in [
            "data-bytes=\"10485760\"",
            "data-total=\"41943040\"",
            "data-rate=\"1048576\"",
            &format!("data-at=\"{AT}\""),
        ] {
            assert!(html.contains(attr), "{attr}: {html}");
        }
        assert!(
            html.contains("value=\"10485760\" max=\"41943040\" aria-label=\"download\""),
            "{html}"
        );
        assert!(
            html.contains("<span data-kit-done>10.0 MB</span> of 40.0 MB"),
            "{html}"
        );
        assert!(html.contains("<span>1.0 MB/s</span>"), "{html}");
        assert!(
            html.contains("<span data-kit-left>30s left</span>"),
            "{html}"
        );
        assert!(html.contains("window.kitTransfer ??="), "{html}");
        assert!(!html.contains("data-every"), "{html}");
    }

    #[test]
    fn the_same_reading_renders_the_same() {
        let draw = || Transfer::new(5_u64, 10_u64).rate(1_u64, AT).to_string();
        assert_eq!(draw(), draw());
    }

    #[test]
    fn a_still_transfer_has_no_rate_or_instant() {
        let html = markup(&Transfer::new(5_u64, 10_u64).rate(0_u64, AT));
        assert!(html.contains("data-rate=\"0\" data-at=\"0\""), "{html}");
        assert!(!html.contains("/s<"), "{html}");
        assert!(!html.contains("data-kit-left"), "{html}");
        assert!(
            html.contains("<span data-kit-done>5 B</span> of 10 B"),
            "{html}"
        );
    }

    #[test]
    fn an_unknown_total_waits_and_counts_nothing_down() {
        let html = markup(&Transfer::new(2048_u64, 0_u64).rate(1024_u64, AT));
        assert!(
            html.contains("<progress class=\"meter\"></progress>"),
            "{html}"
        );
        assert!(!html.contains("value="), "{html}");
        assert!(
            html.contains("<span data-kit-done>2.0 KB</span><"),
            "{html}"
        );
        assert!(!html.contains(" of "), "{html}");
        assert!(!html.contains("data-kit-left"), "{html}");
    }

    #[test]
    fn a_slower_report_moves_for_longer() {
        let html = Transfer::new(5_u64, 10_u64)
            .rate(1_u64, AT)
            .every(Duration::from_secs(5))
            .to_string();
        assert!(html.contains("data-every=\"5000\""), "{html}");
    }

    #[test]
    fn a_tone_reaches_the_meter() {
        let html = Transfer::new(5_u64, 10_u64).tone(Tone::Warn).to_string();
        assert!(html.contains("<progress class=\"meter warn\""), "{html}");
    }

    #[test]
    fn the_mover_stops_at_the_total_a_still_rate_and_two_reports_on() {
        assert!(MOVE.starts_with("(window.kitTransfer ??="));
        assert!(MOVE.contains("2 * (+d.every || 1000)"));
        assert!(MOVE.contains("Math.min(n, v)"));
        assert!(MOVE.contains("r > 0 && s < h && !(n && x >= n)"));
        assert!(
            MOVE.contains("performance.now()"),
            "timed on the page's own clock"
        );
        assert!(!MOVE.contains("Date.now"), "never the reader's wall clock");
        assert_eq!(MOVE.matches("new MutationObserver").count(), 1);
        assert!(
            !MOVE.contains('"'),
            "the expression sits in a double-quoted attribute"
        );
    }
}
