import Foundation
import TVServices

/// The Top Shelf: the records the app last left in `TopShelfSnapshot`. Choosing
/// one opens its page in kōan; Play plays it.
final class ContentProvider: TVTopShelfContentProvider {
    override func loadTopShelfContent(completionHandler: @escaping ((any TVTopShelfContent)?) -> Void) {
        completionHandler(Self.content())
    }

    /// Nothing, while there is no snapshot or it names nothing whose sleeve is
    /// still on disk: the system then shows the static banner.
    private static func content() -> (any TVTopShelfContent)? {
        guard let directory = TopShelfSnapshot.directory,
              let snapshot = TopShelfSnapshot.read(from: directory)
        else { return nil }
        let sections = snapshot.sections.compactMap { section -> TVTopShelfItemCollection<TVTopShelfSectionedItem>? in
            let items = section.items.compactMap { item(for: $0, in: directory, section: section.title) }
            guard !items.isEmpty else { return nil }
            let collection = TVTopShelfItemCollection(items: items)
            collection.title = section.title
            return collection
        }
        return sections.isEmpty ? nil : TVTopShelfSectionedContent(sections: sections)
    }

    private static func item(
        for record: TopShelfSnapshot.Item, in directory: URL, section: String
    ) -> TVTopShelfSectionedItem? {
        let image = directory.appending(path: record.image)
        guard FileManager.default.fileExists(atPath: image.path),
              let open = URL(string: "koan://album/\(record.uid)"),
              let play = URL(string: "koan://album/\(record.uid)?play")
        else { return nil }
        // A record can be on both shelves; identifiers are per section, by
        // title, so one stays put when the other section empties.
        let item = TVTopShelfSectionedItem(identifier: "\(section)-\(record.uid)")
        item.title = record.title
        item.imageShape = .square
        item.setImageURL(image, for: [.screenScale1x, .screenScale2x])
        item.displayAction = TVTopShelfAction(url: open)
        item.playAction = TVTopShelfAction(url: play)
        return item
    }
}
