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
        case .lastPlayed: "lastPlayed"
        case .downloaded: "downloaded"
        }
    }

    init?(storageKey: String) {
        switch storageKey {
        case "recentlyAdded": self = .recentlyAdded
        case "title": self = .title
        case "artist": self = .artist
        case "year": self = .year
        case "random": self = .random
        case "lastPlayed": self = .lastPlayed
        case "downloaded": self = .downloaded
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
        case .lastPlayed: "Last Played"
        case .downloaded: "Most Downloaded"
        }
    }

    /// The sorts to offer. Last Played only means something, and is only
    /// offered, while the Recently Played filter is on; Most Downloaded while
    /// the Downloaded one is.
    static func offered(recent: Bool, downloaded: Bool) -> [AlbumSort] {
        (recent ? [.lastPlayed] : []) + (downloaded ? [.downloaded] : [])
            + [.recentlyAdded, .title, .artist, .year, .random]
    }
}

extension TrackBrowseSort {
    var storageKey: String {
        switch self {
        case .artist: "artist"
        case .title: "title"
        case .album: "album"
        case .duration: "duration"
        case .lastPlayed: "lastPlayed"
        }
    }

    init?(storageKey: String) {
        switch storageKey {
        case "artist": self = .artist
        case "title": self = .title
        case "album": self = .album
        case "duration": self = .duration
        case "lastPlayed": self = .lastPlayed
        default: return nil
        }
    }

    var label: String {
        switch self {
        case .artist: "Artist"
        case .title: "Title"
        case .album: "Album"
        case .duration: "Duration"
        case .lastPlayed: "Last Played"
        }
    }

    /// As `AlbumSort.offered`.
    static func offered(recent: Bool) -> [TrackBrowseSort] {
        (recent ? [.lastPlayed] : []) + [.artist, .title, .album, .duration]
    }
}
