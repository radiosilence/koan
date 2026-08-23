import Foundation

/// Navigation values are distinct types, never raw IDs.
///
/// A `NavigationStack` matches destinations by type, so two `Int64` routes in
/// one stack collide silently and send you to whichever was registered first.
struct AlbumRoute: Hashable {
    let id: Int64
}

struct ArtistRoute: Hashable {
    let id: Int64
}

import KoanFFI

/// Stable strings for `UserDefaults`, so the stored preference survives the
/// enum gaining or reordering cases.
extension AlbumSort {
    var storageKey: String {
        switch self {
        case .recentlyAdded: "recentlyAdded"
        case .title: "title"
        case .artist: "artist"
        case .year: "year"
        case .random: "random"
        }
    }

    init?(storageKey: String) {
        switch storageKey {
        case "recentlyAdded": self = .recentlyAdded
        case "title": self = .title
        case "artist": self = .artist
        case "year": self = .year
        case "random": self = .random
        default: return nil
        }
    }

    var label: String {
        switch self {
        case .recentlyAdded: "Recently Added"
        case .title: "Title"
        case .artist: "Artist"
        case .year: "Year"
        case .random: "Random"
        }
    }

    static let all: [AlbumSort] = [.recentlyAdded, .title, .artist, .year, .random]
}
