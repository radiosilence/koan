//! A glyph answering a press with a motion that says what its verb does:
//! queue drops, share lifts, settings wiggles, add pops, remove wipes away,
//! rescan turns, back nudges back. One program moves the icon's own raster,
//! so any icon answers without its strokes written out again; the
//! showpieces whose parts move apart (play and pause, shuffle, repeat, copy,
//! the heart) keep their hand-drawn answers in `GLYPH`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;

use super::ShaderProgram;
use crate::theme::Icon;

/// How a glyph moves when its button is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gesture {
    /// Dips and lands on a line drawn under it: something put somewhere
    /// (queued, added to a playlist, downloaded, saved).
    Drop,
    /// Rises a step and settles: something sent out (shared, exported).
    Lift,
    /// Rocks on its centre: something opened or looked at.
    Wiggle,
    /// Swells past its size and settles: something added or chosen.
    Pop,
    /// Squeezes thin and fades a little, then comes back: something taken
    /// away.
    Wipe,
    /// Turns once round, clockwise: something run again.
    Turn,
    /// Swings a little way and back, anticlockwise (`false`) or clockwise:
    /// undo and redo.
    Swing(bool),
    /// Steps the way it points and back.
    Nudge(Heading),
    /// Play next's triangle steps forward while the lines under it make
    /// room.
    PlayNext,
}

/// Which way a nudge goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Heading {
    Left,
    Right,
    Up,
    Down,
    UpRight,
}

impl Heading {
    fn vector(self) -> (f32, f32) {
        match self {
            Heading::Left => (-1.0, 0.0),
            Heading::Right => (1.0, 0.0),
            Heading::Up => (0.0, -1.0),
            Heading::Down => (0.0, 1.0),
            Heading::UpRight => (0.7071, -0.7071),
        }
    }
}

impl Gesture {
    /// The motion `icon` answers with, or `None` for a glyph answered some
    /// other way: the showpieces drawn in `GLYPH` and the heart. Matched
    /// without a wildcard, so a new icon is given its motion here.
    pub fn of(icon: Icon) -> Option<Gesture> {
        use Gesture::*;
        use Heading::*;
        Some(match icon {
            Icon::Play
            | Icon::Pause
            | Icon::PlayPause
            | Icon::Shuffle
            | Icon::RepeatQueue
            | Icon::RepeatOne
            | Icon::Copy
            | Icon::Check
            | Icon::Favourite
            | Icon::Favourited => return None,
            Icon::PlayNext => PlayNext,
            Icon::Queue
            | Icon::QueueSection
            | Icon::Playlist
            | Icon::Downloads
            | Icon::DownloadNotice
            | Icon::OnDevice
            | Icon::CloudKept
            | Icon::Save
            | Icon::Paste => Drop,
            Icon::Share | Icon::Export | Icon::Cloud | Icon::Airplay => Lift,
            Icon::Add
            | Icon::Select
            | Icon::Selecting
            | Icon::Picked
            | Icon::Unpicked
            | Icon::SelectAll
            | Icon::Success
            | Icon::Expand
            | Icon::NowPlaying
            | Icon::JumpToPlaying
            | Icon::PlayMark => Pop,
            Icon::Remove
            | Icon::Deselect
            | Icon::Clear
            | Icon::Close
            | Icon::ClearField
            | Icon::Subtract
            | Icon::Cut
            | Icon::CloudMissing
            | Icon::Failure
            | Icon::FailureFilled => Wipe,
            Icon::Rescan | Icon::RescanAll | Icon::Sync | Icon::Reshuffle | Icon::Pending => Turn,
            Icon::Undo => Swing(false),
            Icon::Redo => Swing(true),
            Icon::Back | Icon::Previous | Icon::SkipBack | Icon::Sidebar => Nudge(Left),
            Icon::Forward
            | Icon::Next
            | Icon::SkipForward
            | Icon::Disclosure
            | Icon::Move
            | Icon::Choose => Nudge(Right),
            Icon::PreviousField | Icon::MoveUp | Icon::VolumeUp => Nudge(Up),
            Icon::NextField | Icon::MoveDown | Icon::VolumeDown => Nudge(Down),
            Icon::OpenExternal | Icon::Mail => Nudge(UpRight),
            Icon::Sleep
            | Icon::SleepSet
            | Icon::Album
            | Icon::UnknownAlbum
            | Icon::Artist
            | Icon::Track
            | Icon::History
            | Icon::RecentlyPlayed
            | Icon::Library
            | Icon::Search
            | Icon::Lyrics
            | Icon::LyricsText
            | Icon::Shortcuts
            | Icon::Sort
            | Icon::Filters
            | Icon::More
            | Icon::MoreCircled
            | Icon::Folder
            | Icon::FolderUnknown
            | Icon::Warning
            | Icon::WarningFilled
            | Icon::Alert
            | Icon::Info
            | Icon::CloudProblem
            | Icon::Wifi
            | Icon::Offline
            | Icon::NetworkBlocked
            | Icon::Renderer
            | Icon::Speaker
            | Icon::Output
            | Icon::Laptop
            | Icon::Desktop
            | Icon::Display
            | Icon::Phone
            | Icon::Television
            | Icon::Cable
            | Icon::Headphones
            | Icon::Server
            | Icon::Settings
            | Icon::Devices
            | Icon::Account
            | Icon::People
            | Icon::Tuning
            | Icon::Extensions
            | Icon::Appearance => Wiggle,
        })
    }

    /// The program's code for it.
    pub fn code(self) -> f32 {
        match self {
            Gesture::Drop => 1.0,
            Gesture::Lift => 2.0,
            Gesture::Wiggle => 3.0,
            Gesture::Pop => 4.0,
            Gesture::Wipe => 5.0,
            Gesture::Turn => 6.0,
            Gesture::Swing(_) => 7.0,
            Gesture::Nudge(_) => 8.0,
            Gesture::PlayNext => 9.0,
        }
    }

    /// Which way it goes, on the glyph's grid: a nudge's heading, a swing's
    /// turn (1 clockwise).
    pub fn heading(self) -> (f32, f32) {
        match self {
            Gesture::Nudge(heading) => heading.vector(),
            Gesture::Swing(clockwise) => (if clockwise { 1.0 } else { -1.0 }, 0.0),
            _ => (1.0, 0.0),
        }
    }
}

/// Each icon's raster, kept by icon, stroke and device size. A handful of
/// icons at one or two sizes each; made on a glyph's first press.
#[derive(Default)]
struct Rasters(HashMap<(Icon, u32, u32), Arc<RenderImage>>);

impl Global for Rasters {}

/// `icon` beside type of `points`, `side` device pixels square, as an
/// image the program samples: the sprite's own SVG, so its strokes match
/// the icon it stands in for.
pub fn raster(icon: Icon, points: f32, side: f32, cx: &mut App) -> Option<Arc<RenderImage>> {
    let (svg, units) = crate::theme::icons::svg(icon, points);
    let key = (icon, units, side.round() as u32);
    if let Some(image) = cx.try_global::<Rasters>().and_then(|r| r.0.get(&key)) {
        return Some(image.clone());
    }
    let image = cx
        .svg_renderer()
        .render_single_frame(svg.as_bytes(), side / 24.0)
        .ok()?;
    cx.default_global::<Rasters>().0.insert(key, image.clone());
    Some(image)
}

/// The gestures as one program, moving the icon's raster on its 24-point
/// grid. Uniforms as `GLYPH`'s: 0 the gesture, 1 its heading's x, 2 the
/// spill, 3 its heading's y, 4..8 the glyph's colour, 8..12 the accent (a
/// drop's line), 12 the glyph's side, in device pixels; track 0 the time
/// from 0 to 1, track 1 the same on its curve. At either end the raster
/// stands where the icon does.
pub static GESTURE: ShaderProgram = ShaderProgram {
    name: "koan-gesture",
    msl: r#"
float kq_mask(texture2d<float> image, float4 tile, float2 h) {
    if (h.x < 0.0 || h.y < 0.0 || h.x > 24.0 || h.y > 24.0) { return 0.0; }
    return koan_sample(image, tile, h / 24.0).a;
}
float2 kq_turn(float2 g, float a) {
    float2 v = g - float2(12.0, 12.0);
    float cs = cos(a);
    float sn = sin(a);
    return float2(12.0, 12.0) + float2(cs * v.x + sn * v.y, -sn * v.x + cs * v.y);
}
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = clamp(koan_track(u, 0, time), 0.0, 1.0);
    float e = clamp(koan_track(u, 1, time), 0.0, 1.0);
    float k = max(u[12], 1.0) / 24.0;
    float2 g = (position - u[2]) / k;
    float2 c = float2(12.0, 12.0);
    float2 dir = float2(u[1], u[3]);
    float pi = 3.14159265;
    float bump = sin(pi * t);
    float glyph = 0.0;
    float accent = 0.0;
    int kind = int(u[0] + 0.5);
    if (kind == 1) {
        glyph = kq_mask(image, tile, g - float2(0.0, 2.5 * bump));
        float reach = 9.0 * e;
        float d = max(abs(g.y - 23.0) - 0.6, abs(g.x - 12.0) - reach);
        accent = clamp(-d * k + 0.5, 0.0, 1.0) * sin(pi * min(t * 1.3, 1.0));
    } else if (kind == 2) {
        glyph = kq_mask(image, tile, g + float2(0.0, 3.0 * bump));
    } else if (kind == 3) {
        glyph = kq_mask(image, tile, kq_turn(g, 0.3 * sin(3.0 * pi * t) * (1.0 - t)));
    } else if (kind == 4) {
        float s = 1.0 + 0.3 * bump * (1.0 - 0.4 * t);
        glyph = kq_mask(image, tile, c + (g - c) / s);
    } else if (kind == 5) {
        float sx = max(1.0 - 0.6 * bump, 0.05);
        glyph = kq_mask(image, tile, c + (g - c) / float2(sx, 1.0)) * (1.0 - 0.5 * bump);
    } else if (kind == 6) {
        glyph = kq_mask(image, tile, kq_turn(g, 2.0 * pi * e));
    } else if (kind == 7) {
        glyph = kq_mask(image, tile, kq_turn(g, 0.9 * bump * dir.x));
    } else if (kind == 8) {
        glyph = kq_mask(image, tile, g - dir * 2.5 * bump);
    } else if (kind == 9) {
        float2 tri = g - float2(4.0 * bump, 0.0);
        if (tri.x < 10.5 && tri.y < 11.5) { glyph = kq_mask(image, tile, tri); }
        float2 lines = g - float2(0.0, 1.5 * bump);
        if (!(lines.x < 10.5 && lines.y < 11.5)) { glyph = max(glyph, kq_mask(image, tile, lines)); }
    }
    float la = glyph * u[7];
    float aa = accent * u[11];
    float a = max(la, aa);
    if (a <= 0.0) { return float4(0.0); }
    float3 rgb = mix(float3(u[8], u[9], u[10]), float3(u[4], u[5], u[6]), la / (la + aa));
    return float4(rgb, a);
}
"#,
    wgsl: r#"
fn kq_mask(tile: vec4<f32>, h: vec2<f32>) -> f32 {
    if (h.x < 0.0 || h.y < 0.0 || h.x > 24.0 || h.y > 24.0) { return 0.0; }
    return koan_sample(tile, h / 24.0).a;
}
fn kq_turn(g: vec2<f32>, a: f32) -> vec2<f32> {
    let v = g - vec2<f32>(12.0, 12.0);
    let cs = cos(a);
    let sn = sin(a);
    return vec2<f32>(12.0, 12.0) + vec2<f32>(cs * v.x + sn * v.y, -sn * v.x + cs * v.y);
}
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = clamp(koan_track(u, 0u, time), 0.0, 1.0);
    let e = clamp(koan_track(u, 1u, time), 0.0, 1.0);
    let k = max(u[3].x, 1.0) / 24.0;
    let g = (position - u[0].z) / k;
    let c = vec2<f32>(12.0, 12.0);
    let dir = vec2<f32>(u[0].y, u[0].w);
    let pi = 3.14159265;
    let bump = sin(pi * t);
    var glyph = 0.0;
    var accent = 0.0;
    let kind = i32(u[0].x + 0.5);
    if (kind == 1) {
        glyph = kq_mask(tile, g - vec2<f32>(0.0, 2.5 * bump));
        let reach = 9.0 * e;
        let d = max(abs(g.y - 23.0) - 0.6, abs(g.x - 12.0) - reach);
        accent = clamp(-d * k + 0.5, 0.0, 1.0) * sin(pi * min(t * 1.3, 1.0));
    } else if (kind == 2) {
        glyph = kq_mask(tile, g + vec2<f32>(0.0, 3.0 * bump));
    } else if (kind == 3) {
        glyph = kq_mask(tile, kq_turn(g, 0.3 * sin(3.0 * pi * t) * (1.0 - t)));
    } else if (kind == 4) {
        let s = 1.0 + 0.3 * bump * (1.0 - 0.4 * t);
        glyph = kq_mask(tile, c + (g - c) / s);
    } else if (kind == 5) {
        let sx = max(1.0 - 0.6 * bump, 0.05);
        glyph = kq_mask(tile, c + (g - c) / vec2<f32>(sx, 1.0)) * (1.0 - 0.5 * bump);
    } else if (kind == 6) {
        glyph = kq_mask(tile, kq_turn(g, 2.0 * pi * e));
    } else if (kind == 7) {
        glyph = kq_mask(tile, kq_turn(g, 0.9 * bump * dir.x));
    } else if (kind == 8) {
        glyph = kq_mask(tile, g - dir * 2.5 * bump);
    } else if (kind == 9) {
        let tri = g - vec2<f32>(4.0 * bump, 0.0);
        if (tri.x < 10.5 && tri.y < 11.5) { glyph = kq_mask(tile, tri); }
        let lines = g - vec2<f32>(0.0, 1.5 * bump);
        if (!(lines.x < 10.5 && lines.y < 11.5)) { glyph = max(glyph, kq_mask(tile, lines)); }
    }
    let la = glyph * u[1].w;
    let aa = accent * u[2].w;
    let a = max(la, aa);
    if (a <= 0.0) { return vec4<f32>(0.0); }
    let rgb = mix(u[2].xyz, u[1].xyz, la / (la + aa));
    return vec4<f32>(rgb, a);
}
"#,
    prepare: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon but the hand-drawn showpieces and the heart answers with
    /// a gesture, each a code the program knows.
    #[test]
    fn every_icon_moves_somehow() {
        let mut moving = 0;
        for icon in Icon::ALL {
            match Gesture::of(*icon) {
                Some(g) => {
                    moving += 1;
                    assert!((1.0..=9.0).contains(&g.code()), "{icon:?}");
                }
                None => assert!(
                    super::super::answer::morphs(*icon)
                        || matches!(icon, Icon::PlayPause | Icon::Favourite | Icon::Favourited),
                    "{icon:?} answers with nothing"
                ),
            }
        }
        assert!(moving > 90);
    }

    #[test]
    fn verbs_move_as_they_mean() {
        assert_eq!(Gesture::of(Icon::Queue), Some(Gesture::Drop));
        assert_eq!(Gesture::of(Icon::Share), Some(Gesture::Lift));
        assert_eq!(Gesture::of(Icon::PlayNext), Some(Gesture::PlayNext));
        assert_eq!(Gesture::of(Icon::Back), Some(Gesture::Nudge(Heading::Left)));
        assert_eq!(Gesture::Swing(false).heading().0, -1.0);
    }
}
