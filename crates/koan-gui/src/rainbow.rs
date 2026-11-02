//! The hidden appearance mode, the Mac's `Rainbow`: the accent cycling
//! through an acid hyperpop palette (hot pink, cyber blue, holographic lilac,
//! brat lime; chrome has no hue to carry and sits out), the wash a sheen
//! through it. Hidden on purpose: no Settings row, no docs. The Konami code
//! (↑↑↓↓←→←→BA, outside a text field) switches it, and it is kept per device
//! (`appearance.rainbow`). A Charli XCX track brings it out for itself one
//! time in four, in brat's lime, and puts it away when the next track starts.
//!
//! Off, it costs nothing. On, its one timer, the accent's step every two
//! seconds, runs only while a window of the app is in front and motion is
//! allowed.

use std::time::Duration;

use koan_ffi::QueueItem;
use koan_kit::app::*;

use crate::engine::engine;
use crate::mirror::Mirror;
use crate::theme::wash::Sleeve;
use crate::theme::{Accent, KoanTheme, Tone};

/// Hot pink, cyber blue, holographic lilac, brat lime.
const HYPERPOP: [u32; 4] = [0xFF2E9A, 0x00C8FF, 0xB388FF, 0x8ACE00];
/// Brat: lime, a little lighter and a little darker.
const BRAT: [u32; 3] = [0x8ACE00, 0xB0F02A, 0x6FA800];
/// How long the accent holds each colour.
const PERIOD: Duration = Duration::from_secs(2);

/// ↑↑↓↓←→←→BA.
const CODE: [&str; 10] = [
    "up", "up", "down", "down", "left", "right", "left", "right", "b", "a",
];

const ON: [&str; 15] = [
    "gay mode: ON ✦ it's giving",
    "slay ✧˖°",
    "ur playlist ate",
    "girl what is this song 💅",
    "hyperpop mode unlocked ⋆｡°✩",
    "no thoughts just vibes",
    "mother is mothering",
    "the gays are fighting in the comments",
    "who let her cook 🍳",
    "flop era cancelled",
    "this is her renaissance",
    "chart position: iconic",
    "serving cunt, sorry, i mean content 💅",
    "ok diva",
    "pop girlies stay winning",
];
const OFF: [&str; 2] = [
    "ok back to being normal i guess",
    "and the music stops… for now",
];
const BRAT_TOAST: &str = "brat summer forever 💚";

#[derive(Default)]
pub struct Rainbow {
    /// Switched on by the code, and kept.
    on: bool,
    /// Brought out by the track playing.
    for_track: bool,
    /// The track it last looked at, so each is decided once.
    track: Option<String>,
    /// Where the accent is around the palette.
    step: usize,
    /// A window of the app is in front.
    active: bool,
    /// The last keys pressed, against `CODE`.
    recent: Vec<String>,
    cycle: Option<Task<()>>,
}

impl Global for Rainbow {}

/// What the windows paint: nothing of the rainbow, its own palette, or
/// brat's. Changed only when that changes, so a window's root, which
/// observes it, is not woken by a key pressed or a step of the cycle.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct Painted(pub Option<bool>);

impl Global for Painted {}

impl Rainbow {
    /// Whether the rainbow is drawn: switched on, or brought out by a track.
    pub fn drawn(cx: &App) -> bool {
        cx.try_global::<Rainbow>()
            .is_some_and(|r| r.on || r.for_track)
    }

    fn palette(&self) -> &'static [u32] {
        if self.for_track && !self.on {
            &BRAT
        } else {
            &HYPERPOP
        }
    }

    /// The wash for it: a diagonal sheen through the palette, which the wash
    /// blurs, tones and drifts as it would a record's sleeve.
    pub fn sheen(cx: &App) -> Option<Sleeve> {
        let colours = cx.try_global::<Rainbow>()?.palette();
        const SIDE: u32 = 64;
        let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let t = (x + y) as f32 / (2 * SIDE - 2) as f32 * (colours.len() - 1) as f32;
                let (i, f) = (t.floor() as usize, t.fract());
                let (a, b) = (
                    channels(colours[i]),
                    channels(colours[(i + 1).min(colours.len() - 1)]),
                );
                for c in 0..3 {
                    rgba.push((a[c] as f32 * (1.0 - f) + b[c] as f32 * f) as u8);
                }
                rgba.push(255);
            }
        }
        Sleeve::new(rgba.into(), SIDE, SIDE)
    }
}

fn channels(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

fn accent(hex: u32) -> Accent {
    let [r, g, b] = channels(hex);
    Accent::of(Some([r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0]))
}

/// One of `lines`, chosen by the clock: random enough for a toast.
fn any(lines: &[&'static str]) -> &'static str {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize);
    lines[nanos % lines.len()]
}

/// Whether a track is Charli XCX's, as its artist or album artist, whatever
/// the case and punctuation.
fn is_charli(item: &QueueItem) -> bool {
    let folded = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    [&item.artist, &item.album_artist]
        .iter()
        .any(|name| folded(name).contains("charlixcx"))
}

/// Reads the kept switch, and listens for the code and for the tracks that
/// bring it out.
pub fn install(cx: &mut App) {
    let on = engine(cx).appearance().rainbow;
    cx.set_global(Rainbow {
        on,
        active: true,
        ..Default::default()
    });
    cx.set_global(Painted::default());
    cx.observe_keystrokes(|event, _, cx| {
        // Typing in a field is typing, and a key with a modifier is a
        // shortcut: neither is part of a code, and either starts it over.
        let typing = event.context_stack.iter().any(|c| c.contains("Input"));
        if typing || event.keystroke.modifiers.modified() {
            cx.update_global::<Rainbow, _>(|r, _| r.recent.clear());
            return;
        }
        let key = event.keystroke.key.clone();
        let done = cx.update_global::<Rainbow, _>(|r, _| {
            r.recent.push(key);
            if r.recent.len() > CODE.len() {
                r.recent.remove(0);
            }
            let done = r.recent.iter().map(String::as_str).eq(CODE);
            if done {
                r.recent.clear();
            }
            done
        });
        if done {
            toggle(cx);
        }
    })
    .detach();
    let playback = cx.global::<Mirror>().playback.clone();
    cx.observe(&playback, |playback, cx| {
        let entry = playback.read(cx).0.entry.clone();
        track_started(entry.as_ref(), cx);
    })
    .detach();
    refresh(cx);
}

/// The code: off if the rainbow is drawn, whoever drew it, and on otherwise.
pub fn toggle(cx: &mut App) {
    let on = !Rainbow::drawn(cx);
    cx.update_global::<Rainbow, _>(|r, _| {
        r.on = on;
        r.for_track = false;
    });
    engine(cx).set_rainbow(on);
    crate::notice::show(any(if on { &ON } else { &OFF }), Tone::Accent, cx);
    refresh(cx);
}

/// A track started: a Charli XCX one brings the rainbow out one time in
/// four unless it is on by choice; any other puts a borrowed one away.
fn track_started(entry: Option<&QueueItem>, cx: &mut App) {
    let id = entry.map(|e| e.queue_item_id.clone());
    let (seen, on, before) = {
        let r = cx.global::<Rainbow>();
        (r.track == id, r.on, r.for_track)
    };
    if seen {
        return;
    }
    let lucky = id.as_deref().is_some_and(|id| {
        id.bytes()
            .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
            % 4
            == 0
    });
    let borrowed = !on && entry.is_some_and(is_charli) && lucky;
    cx.update_global::<Rainbow, _>(|r, _| {
        r.track = id;
        r.for_track = borrowed;
    });
    if borrowed {
        crate::notice::show(BRAT_TOAST, Tone::Accent, cx);
    }
    if borrowed != before {
        refresh(cx);
    }
}

/// A window of the app came to the front or left it: the cycle runs only
/// while one is in front.
pub fn set_active(active: bool, cx: &mut App) {
    if cx.global::<Rainbow>().active != active {
        cx.update_global::<Rainbow, _>(|r, _| r.active = active);
        refresh(cx);
    }
}

/// Starts or stops the accent's cycle to match the rainbow, motion and the
/// windows, and sets the accent now.
fn refresh(cx: &mut App) {
    let drawn = Rainbow::drawn(cx);
    let cycling = drawn && cx.global::<Rainbow>().active && !cx.reduce_motion();
    if drawn {
        let r = cx.global::<Rainbow>();
        let colours = r.palette();
        KoanTheme::set_accent(accent(colours[r.step % colours.len()]), true, cx);
    }
    let running = cx.global::<Rainbow>().cycle.is_some();
    if cycling && !running {
        let task = cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(PERIOD).await;
                let stepped = cx.update(|cx| {
                    if !Rainbow::drawn(cx) {
                        return false;
                    }
                    let next = cx.update_global::<Rainbow, _>(|r, _| {
                        r.step += 1;
                        let colours = r.palette();
                        colours[r.step % colours.len()]
                    });
                    KoanTheme::set_accent(accent(next), true, cx);
                    true
                });
                if !stepped {
                    return;
                }
            }
        });
        cx.update_global::<Rainbow, _>(|r, _| r.cycle = Some(task));
    } else if !cycling && running {
        cx.update_global::<Rainbow, _>(|r, _| r.cycle = None);
    }
    // The windows' roots take the wash, and the record's accent back when it
    // goes.
    let painted = Painted(drawn.then(|| {
        let r = cx.global::<Rainbow>();
        r.for_track && !r.on
    }));
    if cx.try_global::<Painted>() != Some(&painted) {
        cx.set_global(painted);
    }
}
