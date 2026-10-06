#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
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
        #if canImport(AppKit)
        let board = NSPasteboard.general
        board.clearContents()
        board.setString(text, forType: .string)
        if let ids { board.setData(ids, forType: .init(trackType)) }
        #elseif os(tvOS)
        _ = (ids, text)
        #else
        // One item carrying both representations. Setting them separately
        // clears what went before.
        var item: [String: Any] = [UTType.utf8PlainText.identifier: text]
        if let ids { item[trackType] = ids }
        UIPasteboard.general.setItems([item])
        #endif
    }

    /// Plain text, for things that are only ever text — a share link.
    static func write(text: String) {
        #if canImport(AppKit)
        let board = NSPasteboard.general
        board.clearContents()
        board.setString(text, forType: .string)
        #elseif os(tvOS)
        _ = text
        #else
        UIPasteboard.general.string = text
        #endif
    }

    /// Rich text with a plain fallback, so a paste into a mail client keeps
    /// the formatting and a paste anywhere else still reads.
    static func write(html: String, text: String) {
        #if canImport(AppKit)
        let board = NSPasteboard.general
        board.clearContents()
        board.setString(html, forType: .html)
        board.setString(text, forType: .string)
        #elseif os(tvOS)
        _ = (html, text)
        #else
        UIPasteboard.general.setItems([[
            UTType.html.identifier: html,
            UTType.utf8PlainText.identifier: text,
        ]])
        #endif
    }

    static func readTrackIds() -> [Int64] {
        guard let data = trackData(),
              let ids = try? JSONDecoder().decode([Int64].self, from: data)
        else { return [] }
        return ids
    }

    private static func trackData() -> Data? {
        #if canImport(AppKit)
        NSPasteboard.general.data(forType: .init(trackType))
        #elseif os(tvOS)
        nil
        #else
        UIPasteboard.general.data(forPasteboardType: trackType)
        #endif
    }
}
