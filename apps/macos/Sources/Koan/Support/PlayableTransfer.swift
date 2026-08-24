import Foundation
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    /// koan's own drag payload. Also declared in the bundle's Info.plist —
    /// without an exported type declaration the system does not recognise the
    /// identifier and every drop silently does nothing. Drags that leave koan
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

    init(kind: Kind, id: Int64, name: String) {
        self.kind = kind
        self.id = id
        self.name = name
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
    func trackIds(using engine: KoanEngine) async -> [Int64] {
        switch kind {
        case .track:
            return [id]
        case .album:
            return (try? await engine.trackIds(albumId: id, artistId: nil)) ?? []
        case .artist:
            return (try? await engine.trackIds(albumId: nil, artistId: id)) ?? []
        }
    }
}

extension View {
    /// Make this view a drag source for `playable`.
    ///
    /// `.onDrag` rather than `.draggable`: the newer Transferable API does not
    /// take effect on List rows here, while the older item-provider one does.
    /// Same identifier and same JSON on the wire, so `.dropDestination(for:)`
    /// still decodes it.
    func draggablePlayable(_ playable: Playable) -> some View {
        draggableTransfer(PlayableTransfer(playable))
    }

    /// The payload directly, for a view that stands for something playable but
    /// has no `Playable` to hand — an artist name inside an album tile knows an
    /// id and a name and nothing else.
    func draggableTransfer(_ transfer: PlayableTransfer) -> some View {
        onDrag {
            let provider = NSItemProvider()
            provider.registerDataRepresentation(
                forTypeIdentifier: UTType.koanPlayable.identifier,
                visibility: .ownProcess
            ) { completion in
                completion(try? JSONEncoder().encode(transfer), nil)
                return nil
            }
            // So a drag that leaves koan still says something useful.
            provider.registerDataRepresentation(
                forTypeIdentifier: UTType.plainText.identifier,
                visibility: .all
            ) { completion in
                completion(Data(transfer.name.utf8), nil)
                return nil
            }
            return provider
        }
    }
}
