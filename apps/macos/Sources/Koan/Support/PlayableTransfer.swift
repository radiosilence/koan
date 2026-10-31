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
        case track, album, artist, playlist
        /// A file or folder dropped from outside koan; `name` is its path.
        case file
        /// Several playables dragged together, in `items`. One pasteboard
        /// item rather than one each: every item crosses the pasteboard
        /// server, and every target the drag passes reads them all back.
        case selection
    }

    let kind: Kind
    let id: Int64
    let name: String
    /// Where the drag started, when that matters.
    ///
    /// A track dragged out of a playlist is still just a track — the queue
    /// wants its id and nothing else. The playlist it came from reorders it
    /// itself, and leaves it out of any drop that would add it back as a copy.
    var origin: Origin?
    /// Dragged out of the queue. Dropped back on the queue it would only be
    /// added again; see the sidebar's Queue row.
    var fromQueue: Bool?
    /// Its place among the items of one drag. A drop hands its items over in
    /// whatever order they finished loading, so the order they were dragged in
    /// travels with them — see `inDragOrder`.
    var place: Int?
    /// What a `.selection` holds, in drag order.
    var items: [PlayableTransfer]?

    struct Origin: Codable, Hashable {
        let playlistId: Int64
    }

    static var transferRepresentation: some TransferRepresentation {
        CodableRepresentation(contentType: .koanPlayable)
        // So a drag into a text field or another app still says something
        // useful rather than failing silently.
        ProxyRepresentation(exporting: \.name)
        // Files from Finder, so every place that takes a playable takes them
        // too. They are indexed where they lie when the drop resolves.
        ProxyRepresentation(importing: { (url: URL) in
            PlayableTransfer(kind: .file, id: 0, name: url.path)
        })
    }

    init(kind: Kind, id: Int64, name: String, origin: Origin? = nil, fromQueue: Bool? = nil) {
        self.kind = kind
        self.id = id
        self.name = name
        self.origin = origin
        self.fromQueue = fromQueue
    }

    /// One transfer carrying `items`. Rows of one list share where they came
    /// from, so the bundle carries that too, for the drops that refuse them.
    init(bundling items: [PlayableTransfer]) {
        kind = .selection
        id = 0
        name = "\(items.count) items"
        origin = items.first?.origin
        fromQueue = items.first?.fromQueue
        self.items = items
    }

    var key: Playable.Key { Playable.Key(kind: kind, id: id) }

    /// The transfers for a drag of several playables, each knowing its place,
    /// bundled when there is more than one.
    static func dragging(_ playables: [Playable]) -> [PlayableTransfer] {
        playables.enumerated().map { PlayableTransfer($1).placed($0) }.bundled
    }

    func placed(_ place: Int) -> PlayableTransfer {
        var placed = self
        placed.place = place
        return placed
    }

    init(_ playable: Playable) {
        origin = nil
        fromQueue = nil
        place = nil
        items = nil
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
        case .playlist(let playlistId, let playlistName):
            kind = .playlist
            id = playlistId
            name = playlistName
        }
    }

    /// What it names, for the engine to resolve on its command lane. A file
    /// has to be indexed first, and a selection is its items.
    var source: TrackSet? {
        switch kind {
        case .track: .tracks(ids: [id])
        case .album: .album(id: id)
        case .artist: .artist(id: id)
        case .playlist: .playlist(id: id)
        case .file, .selection: nil
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
        case .playlist:
            return (try? await engine.playlistTracks(playlistId: id))?.map(\.track.id) ?? []
        case .file:
            return (try? await engine.importFiles(paths: [name]))?.trackIds ?? []
        case .selection:
            var ids: [Int64] = []
            for item in items ?? [] {
                ids += await item.trackIds(using: engine)
            }
            return ids
        }
    }
}

extension [PlayableTransfer] {
    /// As one `.selection` when there are several.
    var bundled: [PlayableTransfer] {
        count > 1 ? [PlayableTransfer(bundling: self)] : self
    }

    /// Back in the order they were dragged in, selections opened. Items
    /// without a place keep theirs, after the placed ones.
    var inDragOrder: [PlayableTransfer] {
        enumerated()
            .sorted { ($0.element.place ?? .max, $0.offset) < ($1.element.place ?? .max, $1.offset) }
            .flatMap { $0.element.items ?? [$0.element] }
    }

    /// Hand `body` what the drop names, in drag order: at once when it is all
    /// library content, so the command joins the engine's lane in turn with
    /// the rest; after indexing when files came with it, since indexing a
    /// folder takes seconds and must not hold every other command up.
    @MainActor
    func withSources(using engine: KoanEngine, _ body: @escaping @MainActor ([TrackSet]) -> Void) {
        let items = inDragOrder
        let named = items.compactMap(\.source)
        if named.count == items.count {
            if !named.isEmpty { body(named) }
            return
        }
        Task {
            var sources: [TrackSet] = []
            for item in items {
                if let source = item.source {
                    sources.append(source)
                } else {
                    sources.append(.tracks(ids: await item.trackIds(using: engine)))
                }
            }
            body(sources)
        }
    }
}

extension View {
    /// Make this view a drag source for `playable`.
    ///
    /// `.draggable`, not `.onDrag`: the drag recogniser behind it has a movement
    /// threshold, so a press that never moves is still a click. `.onDrag` claims
    /// the press outright and any tap underneath it never fires.
    func draggablePlayable(_ playable: Playable) -> some View {
        #if os(tvOS)
        // Nothing is dragged with a remote.
        self
        #else
        draggable(PlayableTransfer(playable))
        #endif
    }
}
