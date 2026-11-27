//! A state in a tone, and progress toward an end: [`Status`] and [`Meter`].
//!
//! A tone is never the only way to read either. A status's word says what its colour does, and a meter is named for
//! a screen reader.

use std::fmt;

use askama::Template;

use crate::action::Busy;
use crate::arg::{Amount, Flag, Text};
use crate::attrs::{Attrs, component};

/// The tone of a state: fine, a warning, an error, waiting, or settled but unverified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// Connected, done, moving: the theme's mint in every app, whatever its accent.
    Ok,
    /// Needs attention but works.
    Warn,
    /// Failed, refused, broken.
    Bad,
    /// Queued, unknown, not started: muted.
    #[default]
    Wait,
    /// Settled but not verified, such as a connector that is configured but whose health is unknown: in the ink, so
    /// it reads neither as working nor as still being found out.
    Neutral,
}

impl Tone {
    /// The kit's class.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Bad => "bad",
            Self::Wait => "wait",
            Self::Neutral => "neutral",
        }
    }
}

impl fmt::Display for Tone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.class())
    }
}

/// A square mark and a word in a tone: `kit::Status::ok("connected")`.
#[must_use = "a status draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "status.html")]
pub struct Status<'a> {
    word: Text<'a>,
    tone: Tone,
    tone_class: Option<&'static str>,
    busy: bool,
    busy_key: Text<'a>,
    loads: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Status<'a> {
    /// `word` in `tone`.
    pub fn new(tone: Tone, word: impl Into<Text<'a>>) -> Self {
        Self {
            word: word.into(),
            tone,
            tone_class: None,
            busy: false,
            busy_key: Text::default(),
            loads: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// Fine: connected, done, moving.
    pub fn ok(word: impl Into<Text<'a>>) -> Self {
        Self::new(Tone::Ok, word)
    }

    /// Needs attention but works.
    pub fn warn(word: impl Into<Text<'a>>) -> Self {
        Self::new(Tone::Warn, word)
    }

    /// Failed, refused, broken.
    pub fn bad(word: impl Into<Text<'a>>) -> Self {
        Self::new(Tone::Bad, word)
    }

    /// Queued, unknown, not started.
    pub fn wait(word: impl Into<Text<'a>>) -> Self {
        Self::new(Tone::Wait, word)
    }

    /// Settled but not verified: configured, its health unknown.
    pub fn neutral(word: impl Into<Text<'a>>) -> Self {
        Self::new(Tone::Neutral, word)
    }

    /// Still being found out: the mark is the ensō, turning, or under reduced motion standing still.
    pub fn busy(mut self, on: impl Into<Flag>) -> Self {
        self.busy = on.into().get();
        self
    }

    /// Busy while any request of the group `key` names runs, the group its card's buttons join with
    /// [`Button::busy_key`](crate::Button::busy_key): a job's state turns while one of its actions is under way.
    pub fn busy_with(mut self, key: impl Into<Text<'a>>) -> Self {
        self.busy_key = key.into();
        self
    }

    /// The group's signal, when it follows one.
    fn busy_signal(&self) -> Option<Busy> {
        self.busy_key.present().map(Busy::keyed)
    }

    /// Fetches its own replacement from `url` as the page starts, for a state the server takes a moment to learn: the
    /// handler answers with this status, settled, under the same [`id`](Self::id). Busy until it does.
    pub fn loads(mut self, url: impl Into<Text<'a>>) -> Self {
        self.loads = url.into();
        self.busy = true;
        self
    }

    /// A tone of the app's own stylesheet in place of the kit's, such as a server state with a colour of its own:
    /// `.status.unknown { --tone: … }`.
    pub fn tone_class(mut self, class: &'static str) -> Self {
        self.tone_class = Some(class);
        self
    }

    fn tone(&self) -> &'static str {
        self.tone_class.unwrap_or(self.tone.class())
    }
}

component!(Status);

/// Progress toward an end, as a hairline: `kit::Meter::new(done, total)`.
#[must_use = "a meter draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "meter.html")]
pub struct Meter<'a> {
    value: Option<(Amount, Amount)>,
    tone: Option<Tone>,
    label: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Meter<'a> {
    /// `value` of `max` done.
    pub fn new(value: impl Into<Amount>, max: impl Into<Amount>) -> Self {
        Self {
            value: Some((value.into(), max.into())),
            tone: None,
            label: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// Under way with nothing yet to count: a short length of the line moves along it, or under reduced motion it is
    /// the thicker track alone.
    pub fn waiting() -> Self {
        Self {
            value: None,
            tone: None,
            label: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// The done part in a status's tone rather than the accent.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// What it measures, for a screen reader: `"download"`.
    pub fn label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.label = label.into();
        self
    }
}

component!(Meter);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_says_its_tone_in_words() {
        assert_eq!(
            Status::ok("connected").to_string(),
            "<span class=\"status ok\">connected</span>"
        );
        assert_eq!(
            Status::neutral("configured").to_string(),
            "<span class=\"status neutral\">configured</span>"
        );
        let busy = Status::wait("checking").busy(true).to_string();
        assert_eq!(busy, "<span class=\"status wait busy\">checking</span>");
    }

    #[test]
    fn a_status_follows_its_cards_busy_group() {
        let status = Status::wait("queued").busy_with("job-1").to_string();
        let signal = Busy::keyed("job-1");
        assert!(
            status.contains(&format!("data-class:busy=\"${signal}\"")),
            "{status}"
        );
        assert!(
            crate::Button::bordered("retry")
                .post("/jobs/1/retry")
                .busy_key("job-1")
                .to_string()
                .contains(&format!("data-indicator:{signal}")),
            "the buttons raise the signal the status reads"
        );
    }

    #[test]
    fn a_status_busy_by_itself_stays_busy_in_its_group() {
        let status = Status::wait("checking")
            .loads("/s")
            .busy_with("job-1")
            .to_string();
        let signal = Busy::keyed("job-1");
        assert!(
            status.contains(&format!("data-attr:aria-busy=\"true || ${signal} ? ")),
            "{status}"
        );
    }

    #[test]
    fn a_status_that_loads_asks_with_a_constant_expression() {
        let status = Status::wait("checking")
            .loads("/mcp/1/status?x='y'")
            .id("mcp-1")
            .to_string();
        assert!(
            status.contains("data-url=\"/mcp/1/status?x=&#39;y&#39;\""),
            "{status}"
        );
        assert!(
            status.contains("data-init=\"@get(el.dataset.url)\""),
            "{status}"
        );
        assert!(status.contains("aria-busy=\"true\""), "{status}");
    }

    #[test]
    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "the test hands over references as a template does"
    )]
    fn a_meter_counts_or_waits() {
        let (done, total) = (3_u64, 10_u64);
        let meter = Meter::new(&done, &total).label("download").to_string();
        assert_eq!(
            meter,
            "<progress class=\"meter\" value=\"3\" max=\"10\" aria-label=\"download\"></progress>"
        );
        let waiting = Meter::waiting().tone(Tone::Ok).to_string();
        assert_eq!(waiting, "<progress class=\"meter ok\"></progress>");
    }
}
