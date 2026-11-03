//! The lyrics beside the page, for the track playing: the terminal UI's `L`
//! panel and the Mac's inspector. Synced lyrics follow the song, the line
//! being sung larger and kept in view, and a click on a line seeks to it;
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

use koan_ffi::{LyricLine, Lyrics};
use koan_kit::app::*;

use crate::components::{EmptyState, Inspector, ScrollArea, Spinner, SyncedLines, plain_text};
use crate::engine::{command, engine};
use crate::mirror::{Mirror, Playback, Playhead};
use crate::theme::{Icon, KoanTheme, Look};

pub struct LyricsPanel {
    playback: Entity<Playback>,
    playhead: Entity<Playhead>,
    /// What was found, and for which track.
    loaded: Option<(i64, Option<Lyrics>)>,
    /// The track LRCLIB is being asked about.
    fetching: Option<i64>,
    /// The track `_load` reads for. Replacing the load drops the one before,
    /// its fetch included, so a track left and come back to is read again.
    loading: Option<i64>,
    /// The line being sung.
    active: Option<usize>,
    _load: Option<Task<()>>,
    _follow: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl LyricsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut panel = Self {
            _subscriptions: [
                cx.observe(&mirror.playback, |this, _, cx| this.track_changed(cx)),
                cx.observe(&mirror.playhead, |this, _, cx| this.follow(cx)),
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
            _load: None,
            _follow: None,
        };
        panel.track_changed(cx);
        panel
    }

    fn track(&self, cx: &App) -> Option<i64> {
        self.playback
            .read(cx)
            .0
            .entry
            .as_ref()
            .and_then(|e| e.track_id)
    }

    fn lyrics(&self, cx: &App) -> Option<&Lyrics> {
        let (track, lyrics) = self.loaded.as_ref()?;
        (Some(*track) == self.track(cx)).then_some(lyrics.as_ref()?)
    }

    fn track_changed(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.track(cx) else {
            self.loaded = None;
            self._load = None;
            self.loading = None;
            self.fetching = None;
            self._follow = None;
            cx.notify();
            return;
        };
        if self.loading == Some(track) {
            return;
        }
        self.loading = Some(track);
        self.fetching = None;
        self.active = None;
        self._follow = None;
        let engine = engine(cx);
        self._load = Some(cx.spawn(async move |this, cx| {
            let cached = engine.clone().lyrics(track).await.ok().flatten();
            let synced = cached.as_ref().is_some_and(|l| l.synced);
            let alive = this.update(cx, |this, cx| {
                this.loaded = Some((track, cached.clone()));
                this.fetching = (!synced).then_some(track);
                this.follow(cx);
                cx.notify();
            });
            if alive.is_err() || synced {
                return;
            }
            let fetched = engine.fetch_lyrics(track).await.ok().flatten();
            this.update(cx, |this, cx| {
                if this.fetching == Some(track) {
                    this.fetching = None;
                }
                this.loaded = Some((track, fetched.or(cached)));
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
        let anchor = *self.playhead.read(cx);
        let Some(lyrics) = self.lyrics(cx) else {
            return;
        };
        if lyrics.lines.is_empty() {
            return;
        }
        let position = anchor.at(Instant::now(), 0) as f64 / 1000.0;
        let (active, next) = place(&lyrics.lines, position);
        if active != self.active {
            self.active = active;
            cx.notify();
        }
        let (true, Some(wait)) = (anchor.playing, next) else {
            return;
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
            .map(|l| (l.time_secs * 1000.0) as u64)
        else {
            return;
        };
        crate::transport::land(at, cx);
        command(cx, "seek to a line", move |e| e.seek(at));
    }
}

impl Render for LyricsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let washed = KoanTheme::washes_window(cx);
        let track = self.track(cx);
        let loading = self.fetching.is_some() && self.fetching == track;
        let lyrics = self.lyrics(cx).cloned();
        let mut panel = Inspector::new("lyrics").clear(washed);
        if loading {
            panel = panel.trailing(Spinner::new(look.font_size(crate::theme::TypeRole::Fine)));
        } else if let Some(source) = lyrics.as_ref().and_then(|l| l.source.clone()) {
            panel = panel.trailing(source);
        }
        let body = match &lyrics {
            Some(l) if !l.lines.is_empty() => SyncedLines::new(
                "lyrics-lines",
                l.lines.iter().map(|l| l.text.clone().into()).collect(),
                self.active,
            )
            .on_line(cx.listener(|this, line: &usize, _, cx| this.seek_to(*line, cx)))
            .into_any_element(),
            Some(l) if !l.content.is_empty() => {
                ScrollArea::new("lyrics-plain", plain_text(l.content.clone(), &look))
                    .into_any_element()
            }
            _ if loading => div().into_any_element(),
            _ => EmptyState::new(
                Icon::LyricsText,
                if track.is_none() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
