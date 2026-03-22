import Foundation

struct Track: Decodable, Identifiable {
    let id: Int
    let title: String
    let artist: String
    let albumArtist: String?
    let album: String
    let albumId: Int?
    let artistId: Int?
    let disc: Int?
    let trackNumber: Int?
    let durationMs: Int?
    let codec: String?
    let sampleRate: Int?
    let bitDepth: Int?
    let channels: Int?
    let bitrate: Int?
    let genre: String?
    let source: String?
    let remoteId: String?
    let isFavourite: Bool?

    var durationFormatted: String {
        guard let ms = durationMs else { return "--:--" }
        let totalSeconds = ms / 1000
        let minutes = totalSeconds / 60
        let seconds = totalSeconds % 60
        return String(format: "%d:%02d", minutes, seconds)
    }
}
