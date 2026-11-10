import KoanFFI
import SwiftUI

extension EnvironmentValues {
    /// The colour the room is wearing — what `.tint` was set to, readable.
    ///
    /// SwiftUI offers no way to read a tint back, and a layer-backed view
    /// drawing in it has to be handed the colour. Set beside the tint, by the
    /// same modifier, so the two cannot disagree.
    @Entry var roomTint: Color = .koanAccent
    /// Drawn by the evidence renderer, in a window no scene manages.
    @Entry var drawnOffscreen = false
}

/// The room around the page: the wash on the window and the tint on the
/// controls, which are the same answer.
///
/// A modifier rather than lines in the shell's body, because what it reads
/// moves per track — the record playing, the page you are on — and a read in
/// the root body is charged to the root, which re-runs the window and rebuilds
/// the toolbar with it, throwing away the filter field and the focus in it. A
/// modifier's body is its own. `content` is the window already built, and the
/// tint reaches it as an environment change that only what draws in it sees.
///
/// The record the room takes its colour from: a page about one record answers
/// with it — an album with its own sleeve, a playlist with the first of its
/// records, the same one that leads its mosaic. Every other page — a grid, a
/// list of artists, favourites, history — is not about any record in
/// particular, so it answers with the one playing. The room is coloured by the
/// music wherever you have wandered off to, and only a page that disagrees
/// says otherwise.
struct RecordRoom: ViewModifier {
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(CoverArtCache.self) private var art
    /// Read for the wash a playlist page sits in: its colour is the first
    /// record in it, since a playlist has no cover of its own.
    @Environment(PlaylistsModel.self) private var playlists
    @Environment(\.drawnOffscreen) private var offscreen
    @Environment(AppearanceModel.self) private var appearance
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(PlayingLevels.self) private var levels: PlayingLevels?

    /// The colour of a record the cache could not already answer for, and which
    /// record it was worked out for. Only consulted when the cache cannot.
    @State private var fetchedTint: (source: AlbumArtwork.Source, colour: Color?)?
    /// The colour the room is wearing, kept on while the next one is worked
    /// out. Falling back to the accent instead flashes every tinted control to
    /// it and back again on the way to a record whose colour is not in yet.
    @State private var worn: Color?
    /// Where the rainbow's accent is around the palette (see `Rainbow`).
    @State private var rainbowStep = 0
    /// Whether the app is in front. Followed by notification rather than
    /// `scenePhase`, which read here re-runs the whole scene.
    @State private var active = true

    /// Only for a colour that had to be worked out, which arrives after the page
    /// and would otherwise cut. A colour already in hand needs no ease: it lands
    /// in the same frame as the record it belongs to, which is what an ease was
    /// standing in for.
    ///
    /// It is deliberately not on the common path. A tint is a value every
    /// control reads rather than a property of a layer, so the compositor
    /// cannot take this one — easing it over two seconds is a hundred and
    /// twenty renders of the whole window, each one a commit, and each commit a
    /// synchronous round trip to the render server.
    ///
    /// The kōan theme eases for a quarter of a second (`Motion.settle`): its
    /// accent is the colour of every selection and indicator, and a two-second
    /// drift there reads as something wrong rather than a room changing.
    @MainActor private static var tintEase: Animation {
        KoanTheme.metric(KoanTheme.Motion.settle, system: .easeInOut(duration: 2))
    }

    /// Read straight through the cache on every pass, the way `AlbumArtwork`
    /// reads its bitmap: a colour the app already holds lands in the same commit
    /// as the page that wanted it. Held in `@State` and written by a task, it
    /// would be a second commit every time — the page, and then the room around it.
    ///
    /// Doubly optional, as `ArtworkBleed.answered` is: the outer `nil` is a
    /// colour not worked out yet, the inner one a record with none.
    private var recordTint: Color?? {
        guard let colourSource else { return .some(nil) }
        if let held = art.cachedColour(for: colourSource) { return held }
        guard let fetchedTint, fetchedTint.source == colourSource else { return nil }
        return .some(fetchedTint.colour)
    }

    /// The record's colour once it is known, and until then the one already
    /// on; `nil` is a record with none, nothing playing, or colours from the
    /// record turned off.
    private var record: Color? {
        guard appearance.recordColours else { return nil }
        return switch recordTint {
        case .some(let colour): colour
        case .none: worn
        }
    }

    /// The accent for that record, tone-mapped to its bands — in either look.
    /// With the rainbow drawn, the palette's, wherever its cycle has reached.
    private var accent: KoanAccent {
        appearance.rainbowDrawn ? .rainbow(rainbowStep) : KoanAccent.of(record)
    }

    /// Whether the rainbow's accent moves round the palette: only while it is
    /// drawn, motion is allowed and the app is in front. Still, it holds one
    /// hue: a phone playing in the background is not woken to change it.
    private var cycles: Bool { appearance.rainbowDrawn && !reduceMotion && active }

    /// The colour to put on.
    private var tint: Color { accent.color }

    private var colourSource: AlbumArtwork.Source? {
        switch nav.current {
        case .album(let id): .album(id)
        case .section(.playlist(let id)): playlists.covers[id]?.first ?? player.currentArtwork
        default: player.currentArtwork
        }
    }

    func body(content: Content) -> some View {
        // The window background is evaluated by the *scene*, outside every
        // environment this was handed, so anything it needs is captured here.
        // Reading an `@Environment` inside that closure — including to put one
        // back — traps, and the app dies on launch.
        let wash = colourSource
        // Whether this pass had to guess. The cache is not observed, so a
        // colour landing in it later — worked out by `LibraryModel.warm`
        // alongside the rows — re-renders nothing; the task below hands it
        // over instead.
        let guessed = recordTint == nil
        let player = player
        let artCache = art
        let appearanceModel = appearance
        let playingLevels = levels
        // Read only with the rainbow drawn: off, the app coming and going
        // re-runs none of this.
        let inFront = !appearance.rainbowDrawn || active
        // Over an opaque ground, because this *replaces* the window's own
        // background rather than sitting on it — a half-transparent wash on its
        // own leaves you looking through the app at the desktop.
        let washLayer = ZStack {
            Rectangle().fill(KoanTheme.ground(.background))
            WindowWash(source: wash, player: player, inFront: inFront)
                .environment(artCache)
                .environment(appearanceModel)
                .environment(playingLevels)
        }

        content
            // The queue is a list of names, and the record playing is the only
            // thing in it with a colour. On the *window* rather than behind
            // the queue: nothing inside a split view column reaches past the
            // toolbar's inset, and a wash that stops in a line under the
            // toolbar is worse than none. An album page washes its own header,
            // so the window stays out of its way.
            // A window on the Mac, the navigation container on iOS: the same
            // intent, and neither platform has the other's container.
            .background { washLayer.ignoresSafeArea() }
            // Only for a record whose colour is not already known. The usual
            // path is answered above, in the same pass as the page —
            // navigating warms this alongside the rows, see
            // `LibraryModel.prepare(album:)`.
            .task(id: colourSource) {
                guard let colourSource else { return }
                if let held = art.cachedColour(for: colourSource) {
                    // Worked out between this pass reading the cache and the
                    // task starting. Without this the room keeps the colour it
                    // was wearing — the accent, on a first visit — for as long
                    // as nothing else happens to redraw it.
                    if guessed { fetchedTint = (colourSource, held) }
                    return
                }
                // Nobody is waiting on a slow ease into the background, so it
                // stands aside until the page in front of it has drawn rather
                // than racing it for artwork, threads and a slot on the main
                // actor.
                try? await Task.sleep(for: .milliseconds(150))
                let colour = await art.dominantColour(for: colourSource)
                guard !Task.isCancelled else { return }
                withAnimation(reduceMotion ? nil : Self.tintEase) { fetchedTint = (colourSource, colour) }
            }
            // Overrides the app-wide tint for everything below, which is every
            // control koan draws itself. What AppKit draws — list selection,
            // focus rings — keeps the declared accent, and that is deliberately
            // a neutral so the two never argue. A television's alerts take the
            // tint too, as text on their white focused button, so there the
            // colour goes to the room alone.
            .environment(\.roomTint, tint)
            .environment(\.koanAccent, accent)
            .environment(\.koanRainbow, appearance.rainbowDrawn)
            .environment(\.koanRainbowStep, rainbowStep)
            // Each step is a new tint, and a tint is read by every control:
            // a whole-window pass every `Rainbow.period`, while the rainbow is
            // drawn. With it off this task is never started.
            .task(id: cycles) {
                guard cycles else { return }
                while !Task.isCancelled {
                    try? await Task.sleep(for: Rainbow.period)
                    guard !Task.isCancelled else { return }
                    Rainbow.step = rainbowStep + 1
                    withAnimation(KoanTheme.Motion.settle) { rainbowStep += 1 }
                }
            }
            .onReceive(NotificationCenter.default.publisher(for: .appResignsActive)) { _ in active = false }
            .onReceive(NotificationCenter.default.publisher(for: .appBecomesActive)) { _ in active = true }
            .background { RainbowForTrack() }
            .overlay { MirrorBall(active: inFront).ignoresSafeArea() }
            .overlay { RainbowBurst().ignoresSafeArea() }
            .overlay(alignment: .bottom) { RainbowToast() }
            // The theme's text button for every button that names no style.
            // Not on a television, whose shell gives them `TelevisionButton`:
            // a bare text button there shows no focus.
            .onChange(of: record, initial: true) { _, now in worn = now }
    }
}

/// The wash behind the window, reading whether anything is playing itself —
/// play and pause change how it breathes and nothing else about the window.
///
/// Handed the model rather than a value taken from it: taken in the shell, the
/// read would be the root's, and every pause would re-run the whole window.
private struct WindowWash: View {
    let source: AlbumArtwork.Source?
    let player: PlayerModel
    let inFront: Bool

    var body: some View {
        ArtworkBleed(source: source, drifts: player.isPlaying, inFront: inFront)
    }
}
