import AppKit

/// Copy and paste of tracks.
///
/// Two representations go on the pasteboard: koan's own track IDs, so a paste
/// back into the queue restores exactly what was copied, and plain text so the
/// same copy is useful in a message or a text file. Anything that isn't koan
/// falls back to reading the text, which we can't turn back into tracks — hence
/// carrying both rather than only the pretty one.
enum Pasteboard {
    static let trackType = NSPasteboard.PasteboardType("cc.blit.koan.track-ids")

    static func write(trackIds: [Int64], text: String) {
        let board = NSPasteboard.general
        board.clearContents()
        board.setString(text, forType: .string)
        if let data = try? JSONEncoder().encode(trackIds) {
            board.setData(data, forType: trackType)
        }
    }

    /// Plain text, for things that are only ever text — a share link.
    static func write(text: String) {
        let board = NSPasteboard.general
        board.clearContents()
        board.setString(text, forType: .string)
    }

    static func readTrackIds() -> [Int64] {
        guard let data = NSPasteboard.general.data(forType: trackType),
              let ids = try? JSONDecoder().decode([Int64].self, from: data)
        else { return [] }
        return ids
    }

    static var hasTracks: Bool {
        NSPasteboard.general.data(forType: trackType) != nil
    }
}
