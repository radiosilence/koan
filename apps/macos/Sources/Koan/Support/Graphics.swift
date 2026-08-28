import SwiftUI

/// How much koan draws.
///
/// One knob rather than a switch per effect, because there is only one thing to
/// decide: how much of the machine the app may spend on looking like itself.
///
/// The steps are ordered by what they were measured to cost, not by how much
/// they look like they cost. On an M1 Pro, playing, window frontmost, mean over
/// thirty seconds:
///
///     full      koan-app 15-18%    the wash drifting
///     reduced   koan-app     8%    the wash held still
///     plain     koan-app     8%    no wash at all
///
/// Which is why `reduced` sits where it does. The wash's blur is rasterised
/// once at 360 points and magnified as a texture, so holding it still costs the
/// same as not drawing it: the record's colour is free, and only the motion is
/// billed. Everything `full` adds over `reduced` is motion, and it is the only
/// step of the three that shows up in a measurement on this machine.
///
/// `plain` is underneath that for machines where drawing a blurred backdrop is
/// dear at all — an older GPU, or a large external display, where none of the
/// numbers above are the ones that matter. It is the only step that stands the
/// glass down, and it does so on that reasoning rather than on evidence from
/// here: held still, glass costs nothing measurable on an M1 Pro.
enum Graphics: Int, CaseIterable, Identifiable {
    /// Nothing the window pays for every frame. Declared first and stored last:
    /// the raw values are what is on disk and cannot move, the order is what
    /// the slider shows.
    case bare = 3
    /// No wash, still indicators, flat chrome.
    case plain = 0
    /// The record's colour behind the window, held still. Everything else as it
    /// is.
    case reduced = 1
    /// The wash drifts, the bars dance, the chrome is glass.
    case full = 2

    var id: Self { self }

    var label: String {
        switch self {
        case .bare: "Bare"
        case .plain: "Plain"
        case .reduced: "Reduced"
        case .full: "Full"
        }
    }

    /// What the setting says about itself, under the slider.
    var detail: String {
        switch self {
        case .bare:
            """
            Everything Plain stands down, and the window's own glass with it:             an opaque toolbar and no soft edge where content passes under the             transport. Those are the platform's, not koan's, and they are             redrawn whenever anything behind them moves.
            """
        case .plain:
            "No colour behind the window, indicators held still, flat chrome instead of glass. For a machine that would rather spend nothing on this."
        case .reduced:
            "The record's colour behind the window, held still — which measures the same as no colour at all. Only the drift is expensive."
        case .full:
            "The colour drifts while something is playing. Around a tenth of a core more than the other two, for as long as the music runs."
        }
    }

    /// Whether the wash is drawn at all.
    var showsWash: Bool { self != .plain && self != .bare }

    /// Whether the wash drifts while something is playing.
    var drifts: Bool { self == .full }

    /// Whether the playing indicators dance. Off, they keep their shape and
    /// stop asking the analyser for levels.
    var animatesIndicators: Bool { self != .plain && self != .bare }

    /// Whether the chrome is glass rather than a flat material.
    var usesGlass: Bool { self != .plain && self != .bare }

    /// Whether the *window* keeps its glass — the toolbar floating over live
    /// content, and the soft edge that fades a row out as it passes under the
    /// transport.
    ///
    /// Separate from `usesGlass`, which is koan's own chrome and the only thing
    /// the setting used to reach. These two are the platform's, they are on at
    /// every other step whatever the setting said, and they are re-rendered
    /// whenever the content behind them changes — which a page switch does
    /// wholesale.
    var usesWindowGlass: Bool { self != .bare }
}

/// Glass where the machine can afford it, a flat material where it cannot.
///
/// The fallback is per site rather than derived: `.clear` glass over artwork and
/// `.regular` glass under a transport bar are not the same thing standing in
/// for the same thing, and only the call site knows what the shape is holding.
private struct AffordableGlass<S: Shape, F: ShapeStyle>: ViewModifier {
    let glass: Glass
    let fallback: F
    let shape: S

    @AppStorage("graphics") private var graphics = Graphics.full

    func body(content: Content) -> some View {
        if graphics.usesGlass {
            content.glassEffect(glass, in: shape)
        } else {
            content.background(fallback, in: shape)
        }
    }
}

extension View {
    func glass(_ glass: Glass, fallback: some ShapeStyle, in shape: some Shape) -> some View {
        modifier(AffordableGlass(glass: glass, fallback: fallback, shape: shape))
    }
}
