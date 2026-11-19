//! The playing indicators: three bars moving with the music, wherever a
//! playing row or record is on screen. koan-kit draws them and keeps their
//! feed (`live_bars`, `install_bars`); this says where the bands come from
//! (the engine's analyser, read through koan-core's `BarHeights`, which the
//! Mac app shares) and draws the renderer's mark in their place while the
//! music plays on a renderer handed the file itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::StreamExt as _;
use koan_kit::app::*;
use koan_kit::components::{BandSource, install_bars, live_bars, renderer_mark};
use parking_lot::Mutex;

use koan_core::audio::viz::{BarHeights, VizLevels};

use crate::engine::engine;

use crate::theme::Hsla;

/// The music plays on a renderer handed the original file, which no bars
/// can follow; with a processed stream, sent from here, they can.
static HANDED_OFF: AtomicBool = AtomicBool::new(false);

/// Follows whether the music is handed off, from the mirror's renderer
/// output and stream format. Every window is drawn again when it changes,
/// which is when an output is chosen.
fn follow_hand_off(cx: &mut App) {
    let Some(mirror) = cx.try_global::<crate::mirror::Mirror>().cloned() else {
        return;
    };
    let update = move |cx: &mut App| {
        let on_renderer = mirror.renderers.read(cx).0.output.is_some();
        let processed = mirror
            .playback
            .read(cx)
            .0
            .format
            .as_ref()
            .is_some_and(|f| f.dsp.is_some());
        let handed = on_renderer && !processed;
        if HANDED_OFF.swap(handed, Ordering::Relaxed) != handed {
            cx.refresh_windows();
        }
    };
    update(cx);
    let (renderers, playback) = {
        let m = cx.global::<crate::mirror::Mirror>();
        (m.renderers.clone(), m.playback.clone())
    };
    let again = update.clone();
    cx.observe(&renderers, move |_, cx| update(cx)).detach();
    cx.observe(&playback, move |_, cx| again(cx)).detach();
}

pub fn install(cx: &mut App) {
    follow_hand_off(cx);
    // Kept across follows, so a bar that comes back stands where it was.
    let heights = Arc::new(Mutex::new(BarHeights::new()));
    let source: BandSource = Arc::new(move |cx: &mut App| {
        let stream = engine(cx).viz_keyframes();
        let heights = heights.clone();
        futures::stream::unfold(stream, |stream| async move {
            let heard = stream.next().await;
            heard.map(|heard| (heard, stream))
        })
        .map(move |heard| {
            heights.lock().take(
                VizLevels {
                    low: heard.low,
                    mid: heard.mid,
                    high: heard.high,
                },
                Instant::now(),
            )
        })
        .boxed()
    });
    install_bars(source, cx);
}

/// The playing bars in `color`. Inside an element with an id, which keys the
/// slot's state.
pub fn bars(color: Hsla) -> AnyElement {
    // Played on a renderer that is handed the file itself, nothing reaches
    // the analyser: the slot shows where it plays instead.
    if HANDED_OFF.load(Ordering::Relaxed) {
        return renderer_mark(color).into_any_element();
    }
    live_bars(color)
        .prism(crate::theme::KoanTheme::prism_now())
        .into_any_element()
}
