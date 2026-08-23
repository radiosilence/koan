import Foundation

/// Playback position, isolated from everything else.
///
/// The engine has no callbacks, so position has to be polled. Keeping it on its
/// own observable means a view can only re-render at that rate if it explicitly
/// asks for the clock — which is the transport, and nothing else. When position
/// lived alongside the rest of the player state, any view that touched that
/// object inherited the tick, and lists were being rebuilt ten times a second
/// for a number they never displayed.
@MainActor
@Observable
final class PlaybackClock {
    private(set) var positionMs: UInt64 = 0
    private(set) var durationMs: UInt64 = 0

    /// Whole seconds, for anything that only needs "roughly where are we" —
    /// lyric highlighting changes once a second, not ten times.
    private(set) var positionSeconds = 0

    /// 0–1 through the current track.
    var progress: Double {
        guard durationMs > 0 else { return 0 }
        return min(1, Double(positionMs) / Double(durationMs))
    }

    func update(positionMs: UInt64, durationMs: UInt64) {
        if positionMs != self.positionMs { self.positionMs = positionMs }
        if durationMs != self.durationMs { self.durationMs = durationMs }
        let seconds = Int(positionMs / 1000)
        if seconds != positionSeconds { positionSeconds = seconds }
    }
}
