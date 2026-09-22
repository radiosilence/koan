import AppKit
import Foundation
import KoanFFI
import Observation

/// The audio behind the playing indicators.
///
/// One subscription for the whole app: the queue and a track list can both
/// have a current row on screen, and they are watching the same music. Nothing
/// here is polled and nothing is timed — the analyser publishes a frame and
/// this wakes on it. When the play head stops and the bars have fallen, it
/// publishes nothing, so this loop is asleep and costs exactly nothing until
/// there is music again.
///
/// Nothing here is observed either. A frame goes straight to the bars drawing
/// it, as layer geometry inside one transaction, and SwiftUI never hears of
/// it. Published through `@Observable`, every frame was a body, a canvas
/// rasterised and a commit — at the display's rate, for as long as music
/// played, for nine points of bar. The rate is still the display's; the cost
/// per frame is now three layer bounds.
///
/// The stream is read only while a bar is attached. With none on screen it is
/// not read at all, and the analyser — which parks when nothing reads it —
/// parks.
///
/// The rate is the refresh rate of the display the window is on, which is
/// something only the window knows: koan sets it here and follows it across
/// screens.
///
/// `Observable` by conformance alone, so it can be handed down the
/// environment. There is no registrar and nothing to notify.
@MainActor
final class PlayingLevels: Observable {
    private let engine: KoanEngine

    /// The bars on screen. Weak, so a row that scrolls away is forgotten
    /// without having to say goodbye.
    private let bars = NSHashTable<PlayingBarsView>.weakObjects()

    /// How high each bar stands, 0...1, low band to high. The spectrum in
    /// three columns: what the analyser says is coming out of the speakers,
    /// and nothing else. Silence is zero and reads flat.
    private var bands = [0.0, 0.0, 0.0]

    private var stamp = Date().timeIntervalSinceReferenceDate
    private var follow: Task<Void, Never>?

    /// The loudest each band has been lately. Each band is judged against its
    /// own recent range rather than against full scale, which is what stops a
    /// track mastered quiet getting a limper indicator than a loud one — and
    /// incidentally undoes the analyser's A-weighting tilt, which otherwise
    /// leaves the bass bar permanently the sluggish one.
    private var ceiling = [quietest, quietest, quietest]

    /// A band below this is room tone, and never sets a ceiling. It is also
    /// what a silent passage is measured against, so silence stays flat rather
    /// than being normalised back up into a dance.
    private static let quietest = 0.12
    /// How long a band takes to forget a loud passage.
    private static let forget = 4.0

    init(engine: KoanEngine) {
        self.engine = engine
        // The rate the analyser should run at is the refresh rate of the
        // display it is drawn on, which changes when the window is dragged to
        // another screen and when a screen is reconfigured under it.
        for name in [
            NSWindow.didChangeScreenNotification,
            NSWindow.didBecomeKeyNotification,
            NSApplication.didChangeScreenParametersNotification,
        ] {
            NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.matchDisplay() }
            }
        }
        matchDisplay()
    }

    deinit { follow?.cancel() }

    /// A bar that wants the music. The first one starts the follow.
    func attach(_ bar: PlayingBarsView) {
        bars.add(bar)
        bar.apply(bands)
        guard follow == nil else { return }
        let stream = engine.vizStream()
        follow = Task { [weak self] in
            while let levels = await stream.next() {
                guard let self, !Task.isCancelled else { return }
                take(levels)
            }
        }
    }

    /// A bar that has stopped listening — off stage, held still, or gone. The
    /// last one to leave ends the follow, and with nothing reading it the
    /// analyser parks.
    func detach(_ bar: PlayingBarsView) {
        bars.remove(bar)
        guard bars.allObjects.isEmpty else { return }
        follow?.cancel()
        follow = nil
    }

    /// A frame, as it arrives. The only smoothing left here is the ceiling
    /// each band is measured against — the fall is the analyser's, which is
    /// also what decays the bars to flat when the music stops.
    private func take(_ levels: VizLevels) {
        let now = Date().timeIntervalSinceReferenceDate
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = min(max(now - stamp, 0), 0.25)
        stamp = now
        let hold = pow(0.5, elapsed / Self.forget)

        let heard = [Double(levels.low), Double(levels.mid), Double(levels.high)]
        var next = bands
        for band in heard.indices {
            ceiling[band] = max(heard[band], max(ceiling[band] * hold, Self.quietest))
            next[band] = min(heard[band] / ceiling[band], 1)
        }
        // A frame that says what the last one said moves nothing. Silence is
        // most of a quiet passage, and it is not worth a commit a frame.
        guard next != bands else { return }
        bands = next
        for bar in bars.allObjects { bar.apply(next) }
    }

    private func matchDisplay() {
        let main = NSApp.windows.first { $0.identifier?.rawValue == MainWindow.id }
        let screen = main?.screen ?? NSApp.keyWindow?.screen ?? NSScreen.main
        engine.setVizFps(fps: UInt8(clamping: screen?.maximumFramesPerSecond ?? 60))
    }
}
