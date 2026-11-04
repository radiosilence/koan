import Foundation

/// What the Top Shelf shows, as the app leaves it for the extension: the
/// extension links no engine and opens no database, and reads only this.
///
/// It lives in the app group's `Library/Caches`, the one place in a group
/// container a tvOS app and its extensions may write. tvOS may empty it; the
/// app writes it again at its next launch, and until then the shelf shows the
/// static banner.
struct TopShelfSnapshot: Codable, Equatable {
    struct Section: Codable, Equatable {
        let title: String
        let items: [Item]
    }

    struct Item: Codable, Equatable {
        /// The record's uid, which `koan://album/<uid>` opens.
        let uid: String
        let title: String
        /// The sleeve's file name, beside the snapshot.
        let image: String
    }

    let sections: [Section]

    static let group = "group.cc.blit.koan"
    static let file = "shelf.json"

    static var directory: URL? {
        FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: group)?
            .appending(path: "Library/Caches/TopShelf", directoryHint: .isDirectory)
    }

    static func read(from directory: URL) -> TopShelfSnapshot? {
        guard let data = try? Data(contentsOf: directory.appending(path: file)) else { return nil }
        return try? JSONDecoder().decode(TopShelfSnapshot.self, from: data)
    }
}
