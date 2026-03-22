import Foundation

struct QueueEntry: Identifiable {
    let queueItemId: String
    let title: String
    let artist: String
    let album: String
    let codec: String?
    let trackNumber: Int?
    let disc: Int?
    let durationMs: UInt64?
    let isCurrent: Bool

    var id: String { queueItemId }

    var durationFormatted: String {
        guard let ms = durationMs else { return "--:--" }
        return formatDurationMs(ms)
    }
}
