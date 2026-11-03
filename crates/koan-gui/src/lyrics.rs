//! The lyrics beside the page, for the track playing: the terminal UI's `L`
//! panel and the Mac's inspector. Synced lyrics follow the song, the line
//! being sung emphasised and kept in view, and a click on a line seeks to it;
//! plain ones are shown whole.
//!
//! The cache answers first, so the panel fills at once; a miss, or a plain
//! copy that may have a synced one upstream by now, asks LRCLIB after,
//! the engine deciding whether that is worth asking again. Words are held
//! with the track they belong to, so a read that loses a race with a track
//! change shows nothing rather than another song.
//!
//! Following the song costs one wake per line: the panel knows every
//! timestamp, so it sets a timer for the next one from the playhead's
//! anchor, and sets nothing while paused. A seek, a pause or a new track
//! moves the anchor, which sets it again.

use std::time::{Duration, Instant};

use koan_kit::app::*;
use koan_runtime::{LyricLine, Lyrics};

use crate::components::{EmptyState, Inspector, ScrollArea, Spinner, SyncedLines, plain_text};
use crate::engine::{command, engine};
use crate::mirror::{Mirror, Playback, Playhead};
use crate::theme::{Icon, KoanTheme, Look};

/// What the words are for: a track of the library, or a queue entry it no
/// longer has, which plays from its own tags.
#[derive(Clone, PartialEq, Hash)]
enum Song {
    Track(i64),
    Entry(String),
}

pub struct LyricsPanel {
    playback: Entity<Playback>,
    playhead: Entity<Playhead>,
    /// What was found, and for which track.
    loaded: Option<(Song, Option<Lyrics>)>,
    /// The track LRCLIB is being asked about.
    fetching: Option<Song>,
    /// The track `_load` reads for. Replacing the load drops the one before,
    /// its fetch included, so a track left and come back to is read again.
    loading: Option<Song>,
    /// The line being sung.
    active: Option<usize>,
    /// When the playhead reached the active line, which its motion is
    /// timed from.
    since: Option<Instant>,
    /// A line clicked to seek to: it stays the line sung while the seek is
    /// on its way (`transport::landing`), so the emphasis moves once rather
    /// than back to the line before on a reading from before the seek.
    sought: Option<usize>,
    _load: Option<Task<()>>,
    _follow: Option<Task<()>>,
    _subscriptions: [Subscription; 4],
}

impl LyricsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut panel = Self {
            _subscriptions: [
                cx.observe(&mirror.playback, |this, _, cx| this.track_changed(cx)),
                cx.observe(&mirror.playhead, |this, _, cx| this.follow(cx)),
                // A seek sent, landing or lapsing.
                cx.observe_global::<crate::transport::Landing>(|this, cx| this.follow(cx)),
                // Cached under the window, and the wash setting decides its
                // ground.
                cx.observe_global::<KoanTheme>(|_, cx| cx.notify()),
            ],
            playback: mirror.playback,
            playhead: mirror.playhead,
            loaded: None,
            fetching: None,
            loading: None,
            active: None,
            since: None,
            sought: None,
            _load: None,
            _follow: None,
        };
        panel.track_changed(cx);
        panel
    }

    fn song(&self, cx: &App) -> Option<Song> {
        let entry = self.playback.read(cx).0.entry.as_ref()?;
        Some(match entry.track_id {
            Some(track) => Song::Track(track),
            None => Song::Entry(entry.queue_item_id.clone()),
        })
    }

    fn lyrics(&self, cx: &App) -> Option<&Lyrics> {
        let (song, lyrics) = self.loaded.as_ref()?;
        (Some(song) == self.song(cx).as_ref()).then_some(lyrics.as_ref()?)
    }

    fn track_changed(&mut self, cx: &mut Context<Self>) {
        let Some(song) = self.song(cx) else {
            self.loaded = None;
            self._load = None;
            self.loading = None;
            self.fetching = None;
            self._follow = None;
            cx.notify();
            return;
        };
        if self.loading.as_ref() == Some(&song) {
            return;
        }
        self.loading = Some(song.clone());
        self.fetching = None;
        self.active = None;
        // A line clicked in the last song is not this one's.
        self.sought = None;
        self._follow = None;
        let engine = engine(cx);
        let Song::Track(track) = song.clone() else {
            let entry = self.playback.read(cx).0.entry.clone();
            self.loaded = Some((song.clone(), None));
            self.fetching = Some(song.clone());
            self._load = Some(cx.spawn(async move |this, cx| {
                let found = match entry {
                    Some(e) => {
                        engine
                            .lyrics_by_tags(e.artist, e.title, e.album, e.duration_ms)
                            .await
                    }
                    None => None,
                };
                this.update(cx, |this, cx| {
                    if this.fetching.as_ref() == Some(&song) {
                        this.fetching = None;
                    }
                    this.loaded = Some((song, found));
                    this.follow(cx);
                    cx.notify();
                })
                .ok();
            }));
            cx.notify();
            return;
        };
        self._load = Some(cx.spawn(async move |this, cx| {
            let cached = engine.clone().lyrics(track).await.ok().flatten();
            let synced = cached.as_ref().is_some_and(|l| l.synced);
            let alive = this.update(cx, |this, cx| {
                this.loaded = Some((song.clone(), cached.clone()));
                this.fetching = (!synced).then(|| song.clone());
                this.follow(cx);
                cx.notify();
            });
            if alive.is_err() || synced {
                return;
            }
            let fetched = engine.fetch_lyrics(track).await.ok().flatten();
            this.update(cx, |this, cx| {
                if this.fetching.as_ref() == Some(&song) {
                    this.fetching = None;
                }
                this.loaded = Some((song, fetched.or(cached)));
                this.follow(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Marks the line being sung and sleeps until the next one starts.
    fn follow(&mut self, cx: &mut Context<Self>) {
        self._follow = None;
        // The seek's target while it is on its way, the engine's playhead
        // once it has landed: never a reading older than the seek.
        let landing = crate::transport::landing(cx);
        let anchor = landing.unwrap_or_else(|| *self.playhead.read(cx));
        let Some(lyrics) = self.lyrics(cx) else {
            return;
        };
        if lyrics.lines.is_empty() {
            return;
        }
        let position = anchor.at(Instant::now(), 0) as f64 / 1000.0;
        let (mut active, next) = place(&lyrics.lines, position);
        // The clicked line held while its seek is on its way; the timer
        // wakes for the landing's lapse, which nothing else may announce.
        // A seek lands within `NEAR_MS` of its target, often a hair short of
        // the line's start, which reads as the line before until the playhead
        // reaches it: the clicked line stays while the playhead is that near.
        let short_of = self
            .sought
            .and_then(|line| lyrics.lines.get(line))
            .map(|line| line.time_secs - position);
        active = sung(active, self.sought, landing.is_some(), short_of);
        let mut held = None;
        if let (Some(_), Some(target)) = (self.sought, landing) {
            held = Some(
                crate::transport::LANDING
                    .saturating_sub(target.at.elapsed())
                    .as_secs_f64(),
            );
        }
        let landed = landing.is_none();
        let now = Instant::now();
        let since = active
            .filter(|_| anchor.playing)
            .and_then(|line| lyrics.lines.get(line))
            .and_then(|line| {
                now.checked_sub(Duration::from_secs_f64(
                    (position - line.time_secs).max(0.0),
                ))
            });
        if landed && !within_reach(short_of) {
            self.sought = None;
        }
        if active != self.active {
            self.active = active;
            self.since = since;
            cx.notify();
        }
        // The next line while playing, and the hold's end either way: a
        // seek that does not land (paused, refused) gives the line back.
        let wait = match (next.filter(|_| anchor.playing), held) {
            (Some(next), Some(held)) => next.min(held),
            (Some(wait), None) | (None, Some(wait)) => wait,
            (None, None) => return,
        };
        self._follow = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f64(wait))
                .await;
            this.update(cx, |this, cx| this.follow(cx)).ok();
        }));
    }

    fn seek_to(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(at) = self
            .lyrics(cx)
            .and_then(|l| l.lines.get(line))
            // Rounded: truncated, `[00:02.01]` lands a millisecond before
            // its line, which then reads as the line before.
            .map(|l| (l.time_secs * 1000.0).round() as u64)
        else {
            return;
        };
        self.sought = Some(line);
        if self.active != Some(line) {
            self.active = Some(line);
            self.since = None;
            cx.notify();
        }
        crate::transport::land(at, cx);
        command(cx, "seek to a line", move |e| e.seek(at));
    }
}

impl Render for LyricsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let washed = KoanTheme::washes_window(cx);
        let song = self.song(cx);
        let loading = self.fetching.is_some() && self.fetching == song;
        let lyrics = self.lyrics(cx).cloned();
        let mut panel = Inspector::new("lyrics").clear(washed);
        if loading {
            panel = panel.trailing(
                Spinner::new(look.font_size(crate::theme::TypeRole::Fine))
                    .announced("lyrics-loading", "looking up lyrics"),
            );
        } else if let Some(source) = lyrics.as_ref().and_then(|l| l.source.clone()) {
            panel = panel.trailing(source);
        }
        let body = match &lyrics {
            Some(l) if !l.lines.is_empty() => SyncedLines::new(
                "lyrics-lines",
                l.lines.iter().map(|l| l.text.clone().into()).collect(),
                self.active,
            )
            .since(self.since)
            // The track: another track's lines start afresh, the same
            // words included.
            .song(&song)
            .on_line(cx.listener(|this, line: &usize, _, cx| this.seek_to(*line, cx)))
            .into_any_element(),
            Some(l) if !l.content.is_empty() => {
                ScrollArea::new("lyrics-plain", plain_text(l.content.clone(), &look))
                    .into_any_element()
            }
            _ if loading => div().into_any_element(),
            _ => EmptyState::new(
                Icon::LyricsText,
                if song.is_none() {
                    "nothing playing"
                } else {
                    "no lyrics found"
                },
            )
            .into_any_element(),
        };
        panel.body(body)
    }
}

/// The line being sung at `position` seconds, the last one started, and how
/// long until the next starts.
fn place(lines: &[LyricLine], position: f64) -> (Option<usize>, Option<f64>) {
    let active = lines.iter().rposition(|l| l.time_secs <= position);
    let next = lines
        .iter()
        .find(|l| l.time_secs > position)
        .map(|l| l.time_secs - position);
    (active, next)
}

/// The line sung: the clicked line while its seek is on its way, so a
/// reading from before the seek cannot move it back a line; the playhead's
/// line otherwise.
fn sung(
    at_playhead: Option<usize>,
    clicked: Option<usize>,
    landing: bool,
    short_of: Option<f64>,
) -> Option<usize> {
    match clicked {
        Some(line) if landing || within_reach(short_of) => Some(line),
        _ => at_playhead,
    }
}

/// Whether the playhead is a hair short of the clicked line's start, as a
/// landed seek leaves it.
fn within_reach(short_of: Option<f64>) -> bool {
    short_of.is_some_and(|s| s > 0.0 && s <= crate::transport::NEAR_MS as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click on line 4 holds it while the seek lands, whatever an older
    /// reading says (line 3), and once landed the playhead decides again.
    #[test]
    fn the_clicked_line_holds_until_the_seek_lands() {
        assert_eq!(sung(Some(3), Some(4), true, None), Some(4));
        assert_eq!(sung(None, Some(4), true, None), Some(4));
        assert_eq!(sung(Some(4), Some(4), false, None), Some(4));
        assert_eq!(sung(Some(5), Some(4), false, None), Some(5));
        assert_eq!(sung(Some(3), None, true, None), Some(3));
    }

    /// A seek that lands a hair before the line's start keeps the clicked
    /// line rather than showing the one before until the playhead arrives.
    #[test]
    fn a_seek_landing_just_short_keeps_the_clicked_line() {
        assert_eq!(sung(Some(3), Some(4), false, Some(0.04)), Some(4));
        assert_eq!(sung(Some(3), Some(4), false, Some(3.0)), Some(3));
        assert_eq!(sung(Some(5), Some(4), false, Some(-1.0)), Some(5));
    }

    #[test]
    fn the_line_sung_is_the_last_one_started_and_the_wait_is_to_the_next() {
        let lines: Vec<LyricLine> = [0.5, 2.5, 7.0]
            .into_iter()
            .map(|time_secs| LyricLine {
                time_secs,
                text: String::new(),
            })
            .collect();
        assert_eq!(place(&lines, 0.0), (None, Some(0.5)));
        assert_eq!(place(&lines, 3.0), (Some(1), Some(4.0)));
        assert_eq!(place(&lines, 9.0), (Some(2), None));
    }
}
