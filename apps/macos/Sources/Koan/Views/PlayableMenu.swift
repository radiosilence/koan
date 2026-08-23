import AppKit
import KoanFFI
import SwiftUI

/// Anything you can put in the queue.
///
/// Tracks, albums and artists differ only in how many tracks they resolve to,
/// so they get one set of actions rather than three near-identical menus that
/// drift apart. Resolution is deferred until an action is chosen — an artist
/// can be thousands of tracks, and building that list to draw a menu would be
/// wasteful and slow.
enum Playable {
    case track(Track)
    case album(Album)
    case artist(id: Int64, name: String)

    var name: String {
        switch self {
        case .track(let t): t.title
        case .album(let a): a.title
        case .artist(_, let name): name
        }
    }

    /// Only things with children are worth shuffling.
    var hasChildren: Bool {
        switch self {
        case .track: false
        case .album, .artist: true
        }
    }

    func trackIds(using engine: KoanEngine) -> [Int64] {
        switch self {
        case .track(let t):
            return [t.id]
        case .album(let a):
            return (try? engine.trackIds(albumId: a.id, artistId: nil)) ?? []
        case .artist(let id, _):
            return (try? engine.trackIds(albumId: nil, artistId: id)) ?? []
        }
    }
}

/// The actions every playable thing offers. Drop into a `.contextMenu`.
struct PlayableMenu: View {
    let playable: Playable

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    var body: some View {
        Button("Play") { act { player.playNow(trackIds: $0) } }
        Button("Play Next") { act { player.playNext(trackIds: $0) } }
        Button("Add to Queue") { act { player.enqueue(trackIds: $0) } }
        if playable.hasChildren {
            Button("Shuffle") { act { player.playNow(trackIds: $0.shuffled()) } }
        }

        Divider()

        Button(favouriteTitle) {
            act { ids in
                for id in ids { player.toggleFavourite(trackId: id) }
                library.refreshFavourites()
            }
        }
        Button("Copy Share Link") { share() }

        if case .track(let track) = playable, let albumId = track.albumId {
            Divider()
            Button("Go to Album") { library.reveal(album: albumId, highlighting: track.id) }
            if let artistId = track.artistId {
                Button("Go to Artist") { library.reveal(artist: artistId) }
            }
        }
        if case .album(let album) = playable {
            Divider()
            Button("Go to Album") { library.reveal(album: album.id) }
            Button("Go to Artist") { library.reveal(artist: album.artistId) }
        }
    }

    /// A single track can say whether it's already a favourite; a collection
    /// can't without resolving it, so it stays a plain verb.
    private var favouriteTitle: String {
        if case .track(let track) = playable {
            return track.isFavourite ? "Remove Favourite" : "Favourite"
        }
        return "Favourite All"
    }

    /// Resolve off the main actor, then act. An artist can be thousands of
    /// tracks and the resolution is a database query.
    private func act(_ body: @escaping @MainActor ([Int64]) -> Void) {
        let engine = library.engine
        let playable = self.playable
        Task {
            let ids = await Task.detached(priority: .userInitiated) {
                playable.trackIds(using: engine)
            }.value
            guard !ids.isEmpty else { return }
            body(ids)
        }
    }

    /// Shares go to the pasteboard rather than a dialog — the only thing you
    /// ever do with a share link is paste it somewhere.
    private func share() {
        let engine = library.engine
        let playable = self.playable
        Task {
            let url = await Task.detached(priority: .userInitiated) { () -> String? in
                let ids = playable.trackIds(using: engine)
                guard !ids.isEmpty else { return nil }
                return (try? engine.createShare(trackIds: ids, description: playable.name)) ?? nil
            }.value

            guard let url else {
                player.report("Couldn't create a share link — local-only tracks can't be shared.")
                return
            }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(url, forType: .string)
            player.report("Share link copied.")
        }
    }
}

/// The big play button on an album or artist page, sat beside the title.
struct PlayableHeaderButton: View {
    let playable: Playable

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var loading = false

    var body: some View {
        Button {
            guard !loading else { return }
            loading = true
            let engine = library.engine
            let playable = self.playable
            Task {
                let ids = await Task.detached(priority: .userInitiated) {
                    playable.trackIds(using: engine)
                }.value
                loading = false
                player.playNow(trackIds: ids)
            }
        } label: {
            ZStack {
                Circle()
                    .fill(.tint)
                    .frame(width: 44, height: 44)
                if loading {
                    ProgressView().controlSize(.small).tint(.white)
                } else {
                    Image(systemName: "play.fill")
                        .font(.system(size: 17))
                        .foregroundStyle(.white)
                        .offset(x: 1)  // optical centring for a triangle
                }
            }
        }
        .buttonStyle(.plain)
        .help("Play \(playable.name)")
    }
}
