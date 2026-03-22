import Foundation

struct NowPlaying {
    let state: PlaybackState
    let positionMs: UInt64
    let durationMs: UInt64?
    let track: NowPlayingTrack?
    let queueItemId: String?

    var positionFormatted: String { formatDurationMs(positionMs) }
    var durationFormatted: String { formatDurationMs(durationMs ?? 0) }

    var progress: Double {
        guard let dur = durationMs, dur > 0 else { return 0 }
        return Double(positionMs) / Double(dur)
    }

    static let empty = NowPlaying(
        state: .stopped, positionMs: 0, durationMs: nil,
        track: nil, queueItemId: nil
    )
}

struct NowPlayingTrack {
    let title: String
    let artist: String
    let album: String
    let codec: String
    let sampleRate: UInt32
    let bitDepth: UInt16
    let channels: UInt16
    let durationMs: UInt64
}
