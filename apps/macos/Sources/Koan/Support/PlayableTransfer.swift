import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    /// koan's own drag payload. Declared in code rather than Info.plist because
    /// nothing outside the app needs to recognise it — drags that leave koan
    /// fall back to the plain-text representation.
    static let koanPlayable = UTType(exportedAs: "cc.blit.koan.playable")
}

/// A dragged playable, as it travels.
///
/// Carries an identity, not a track list: an artist can be thousands of tracks
/// and resolving them to start a drag would stall the gesture. The drop
/// resolves, by which point the user has committed.
struct PlayableTransfer: Codable, Transferable, Hashable {
    enum Kind: String, Codable {
        case track, album, artist
    }

    let kind: Kind
    let id: Int64
    let name: String

    static var transferRepresentation: some TransferRepresentation {
        CodableRepresentation(contentType: .koanPlayable)
        // So a drag into a text field or another app still says something
        // useful rather than failing silently.
        ProxyRepresentation(exporting: \.name)
    }

    init(_ playable: Playable) {
        switch playable {
        case .track(let track):
            kind = .track
            id = track.id
            name = track.title
        case .album(let album):
            kind = .album
            id = album.id
            name = album.title
        case .artist(let artistId, let artistName):
            kind = .artist
            id = artistId
            name = artistName
        }
    }

    /// Resolve to track IDs. Off the main actor — this is a database read, and
    /// an artist is a large one.
    func trackIds(using engine: KoanEngine) -> [Int64] {
        switch kind {
        case .track:
            return [id]
        case .album:
            return (try? engine.trackIds(albumId: id, artistId: nil)) ?? []
        case .artist:
            return (try? engine.trackIds(albumId: nil, artistId: id)) ?? []
        }
    }
}

extension View {
    /// Make this view a drag source for `playable`.
    func draggablePlayable(_ playable: Playable) -> some View {
        draggable(PlayableTransfer(playable))
    }
}
