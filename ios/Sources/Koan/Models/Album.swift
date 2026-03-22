import Foundation

struct Album: Decodable, Identifiable {
    let id: Int
    let title: String
    let artistId: Int
    let artistName: String
    let date: String?
    let codec: String?
    let label: String?
    let trackCount: Int?
    let totalDurationMs: Int?
}
