import Foundation

enum PlaybackState: String, Decodable {
    case stopped = "STOPPED"
    case playing = "PLAYING"
    case paused = "PAUSED"
}

struct NowPlaying: Decodable {
    let state: PlaybackState
    let positionMs: Int
    let durationMs: Int?
    let queueItemId: String?
    let track: NowPlayingTrack?

    var isPlaying: Bool { state == .playing }
    var isPaused: Bool { state == .paused }
    var isStopped: Bool { state == .stopped }
}

struct NowPlayingTrack: Decodable {
    let title: String
    let artist: String
    let album: String
    let codec: String
    let sampleRate: Int
    let bitDepth: Int
    let channels: Int
    let durationMs: Int
}

struct QueueEntry: Decodable, Identifiable {
    let queueItemId: String
    let title: String
    let artist: String
    let album: String
    let codec: String?
    let trackNumber: Int?
    let disc: Int?
    let durationMs: Int?
    let isCurrent: Bool

    var id: String { queueItemId }
}
