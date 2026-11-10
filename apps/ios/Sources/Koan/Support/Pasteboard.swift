import UIKit
import UniformTypeIdentifiers

/// Copy and paste of tracks.
///
/// Two representations go on the pasteboard: koan's own track IDs, so a paste
/// back into the queue restores exactly what was copied, and plain text so the
/// same copy is useful in a message or a text file. Anything that isn't koan
/// falls back to reading the text, which we can't turn back into tracks — hence
/// carrying both rather than only the pretty one.
///
/// tvOS has no pasteboard; there each of these does nothing.
enum Pasteboard {
    static let trackType = "cc.blit.koan.track-ids"

    static func write(trackIds: [Int64], text: String) {
        let ids = try? JSONEncoder().encode(trackIds)
        _ = (ids, text)
    }

    /// Plain text, for things that are only ever text — a share link.
    static func write(text: String) {
        _ = text
    }

    /// Rich text with a plain fallback, so a paste into a mail client keeps
    /// the formatting and a paste anywhere else still reads.
    static func write(html: String, text: String) {
        _ = (html, text)
    }

    static func readTrackIds() -> [Int64] {
        guard let data = trackData(),
              let ids = try? JSONDecoder().decode([Int64].self, from: data)
        else { return [] }
        return ids
    }

    private static func trackData() -> Data? {
        nil
    }
}
