import Foundation

struct Artist: Decodable, Identifiable, Hashable {
    let id: Int64
    let name: String
    let albumCount: Int
    let trackCount: Int
}

struct Album: Decodable, Identifiable, Hashable {
    let id: Int64
    let title: String
    let artistId: Int64
    let artistName: String
    let date: String?
    let codec: String?
    let trackCount: Int
    let totalDurationMs: Int64?

    var formattedDuration: String {
        guard let ms = totalDurationMs else { return "" }
        let totalSeconds = Int(ms / 1000)
        let hours = totalSeconds / 3600
        let minutes = (totalSeconds % 3600) / 60
        if hours > 0 {
            return "\(hours)h \(minutes)m"
        }
        return "\(minutes)m"
    }

    var year: String? {
        date.flatMap { String($0.prefix(4)) }
    }
}

struct Track: Decodable, Identifiable, Hashable {
    let id: Int64
    let title: String
    let artist: String
    let albumArtist: String?
    let album: String
    let albumId: Int64?
    let artistId: Int64?
    let disc: Int?
    let trackNumber: Int?
    let durationMs: Int64?
    let codec: String?
    let sampleRate: Int?
    let bitDepth: Int?
    let genre: String?
    let isFavourite: Bool?

    var formattedDuration: String {
        guard let ms = durationMs else { return "" }
        let totalSeconds = Int(ms / 1000)
        let minutes = totalSeconds / 60
        let seconds = totalSeconds % 60
        return String(format: "%d:%02d", minutes, seconds)
    }
}

struct LibraryStats: Decodable {
    let totalTracks: Int64
    let localTracks: Int64
    let remoteTracks: Int64
    let cachedTracks: Int64
    let totalAlbums: Int64
    let totalArtists: Int64
}
