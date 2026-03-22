import Foundation

struct Artist: Decodable, Identifiable {
    let id: Int
    let name: String
    let albumCount: Int?
    let trackCount: Int?
}
