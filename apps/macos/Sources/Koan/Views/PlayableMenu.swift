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

    func trackIds(using engine: KoanEngine) async -> [Int64] {
        switch self {
        case .track(let t):
            return [t.id]
        case .album(let a):
            return (try? await engine.trackIds(albumId: a.id, artistId: nil)) ?? []
        case .artist(let id, _):
            return (try? await engine.trackIds(albumId: nil, artistId: id)) ?? []
        }
    }
}

/// The actions every playable thing offers. Drop into a `.contextMenu`.
struct PlayableMenu: View {
    let playable: Playable

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(OrganizeModel.self) private var organize
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button("Play") {
            act {
                player.playNow(trackIds: $0)
                nav.showQueueWhenReady(watching: player)
            }
        }
        Button("Play Next") { act { player.playNext(trackIds: $0) } }
        Button("Add to Queue") { act { player.enqueue(trackIds: $0) } }
        if playable.hasChildren {
            Button("Shuffle") {
                act {
                    player.playNow(trackIds: $0.shuffled())
                    nav.showQueueWhenReady(watching: player)
                }
            }
        }

        Divider()

        AddToPlaylistMenu { body in act(body) }

        Divider()

        Button(favouriteTitle) { toggleFavourite() }
        Button("Copy Share Link") { share() }
        Button("Organize Files…") {
            act { ids in
                openWindow(id: OrganizeWindow.id)
                Task { await organize.begin(title: playable.name, trackIds: ids) }
            }
        }

        if case .track(let track) = playable, let albumId = track.albumId {
            Divider()
            Button("Go to Album") { nav.open(album: albumId, highlighting: track.id) }
            if let artistId = track.artistId {
                Button("Go to Artist") { nav.open(artist: artistId) }
            }
        }
        if case .album(let album) = playable {
            Divider()
            Button("Go to Album") { nav.open(album: album.id) }
            Button("Go to Artist") { nav.open(artist: album.artistId) }
        }
    }

    /// An album or an artist is favourited as itself, not as its tracks.
    /// Subsonic stars all three, and starring an album's tracks one by one
    /// would flip the ones already favourited back off.
    private var favouriteTitle: String {
        switch playable {
        case .track(let t):
            return library.isFavourite(track: t.id) ? "Remove Favourite" : "Favourite Track"
        case .album(let a):
            return library.isFavourite(album: a.id) ? "Remove Favourite" : "Favourite Album"
        case .artist(let id, _):
            return library.isFavourite(artist: id) ? "Remove Favourite" : "Favourite Artist"
        }
    }

    private func toggleFavourite() {
        switch playable {
        case .track(let t): library.toggleFavourite(track: t.id)
        case .album(let a): library.toggleFavourite(album: a.id)
        case .artist(let id, _): library.toggleFavourite(artist: id)
        }
    }

    /// Resolve off the main actor, then act. An artist can be thousands of
    /// tracks and the resolution is a database query.
    private func act(_ body: @escaping @MainActor ([Int64]) -> Void) {
        let engine = library.engine
        let playable = self.playable
        Task {
            let ids = await playable.trackIds(using: engine)
            guard !ids.isEmpty else { return }
            body(ids)
        }
    }

    private func share() {
        Share.link(for: playable, engine: library.engine, player: player)
    }
}

/// Creating a share link, in one place: the menu item and the button on an
/// album or artist page must not drift apart.
enum Share {
    /// Asks the remote server for a public link and copies it.
    ///
    /// Only tracks the server knows about can go in a link — it points at the
    /// server, so a local-only file has nothing for it to point at. A mixed
    /// selection shares what it can and says how much it left out. It goes to
    /// the pasteboard because pasting it somewhere is the only thing anyone
    /// does with a share link.
    @MainActor
    static func link(for playable: Playable, engine: KoanEngine, player: PlayerModel) {
        let name = playable.name
        Task {
            let ids = await playable.trackIds(using: engine)
            guard !ids.isEmpty else {
                return deliver(.failure(ShareFailure.nothingToShare), to: player)
            }
            do {
                let share = try await engine.createShare(trackIds: ids, description: name)
                deliver(.success(share), to: player)
            } catch {
                deliver(.failure(error), to: player)
            }
        }
    }

    /// Share a set of tracks directly.
    ///
    /// The queue holds queue items, not library tracks — a queue entry can
    /// outlive the row it came from — so it shares by the track ids it carries
    /// rather than resolving a `Playable`.
    @MainActor
    static func link(trackIds: [Int64], named name: String, engine: KoanEngine, player: PlayerModel) {
        guard !trackIds.isEmpty else {
            player.report("Nothing here can be shared — these tracks aren't in the library.")
            return
        }
        Task {
            do {
                let share = try await engine.createShare(trackIds: trackIds, description: name)
                deliver(.success(share), to: player)
            } catch {
                deliver(.failure(error), to: player)
            }
        }
    }

    private enum ShareFailure: LocalizedError {
        case nothingToShare
        var errorDescription: String? { "these tracks aren't in the library" }
    }

    /// The engine says why it failed, so report that rather than assuming. The
    /// old blanket "local-only tracks can't be shared" sent people looking at
    /// their library when the server had actually refused, or hadn't been
    /// configured at all.
    @MainActor
    private static func deliver(_ result: Result<KoanFFI.Share, Error>, to player: PlayerModel) {
        switch result {
        case .success(let share):
            Pasteboard.write(text: share.url)
            if share.skipped > 0 {
                player.report(
                    "Share link copied — \(share.shared) of \(share.shared + share.skipped) tracks; "
                        + "the rest aren't on your server."
                )
            } else {
                player.report("Share link copied: \(share.url)")
            }
        case .failure(let error):
            player.report("Couldn't create a share link — \(reason(for: error))")
        }
    }

    private static func reason(for error: Error) -> String {
        if case let KoanError.BadArgument(message) = error { return message }
        return error.localizedDescription
    }
}

/// Share button for an album or artist page.
struct ShareButton: View {
    let playable: Playable

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    var body: some View {
        Button {
            Share.link(for: playable, engine: library.engine, player: player)
        } label: {
            Label("Copy Share Link", systemImage: "link")
        }
        .help("Create a public link on your server and copy it")
    }
}

/// The favourite button on a detail page. Labelled, unlike the hover heart on
/// a row — a header has room and the state is worth stating outright.
struct FavouriteHeaderButton: View {
    let playable: Playable

    @Environment(LibraryModel.self) private var library

    private var isOn: Bool {
        switch playable {
        case .track(let t): library.isFavourite(track: t.id)
        case .album(let a): library.isFavourite(album: a.id)
        case .artist(let id, _): library.isFavourite(artist: id)
        }
    }

    var body: some View {
        Button {
            switch playable {
            case .track(let t): library.toggleFavourite(track: t.id)
            case .album(let a): library.toggleFavourite(album: a.id)
            case .artist(let id, _): library.toggleFavourite(artist: id)
            }
        } label: {
            Label(isOn ? "Favourited" : "Favourite", systemImage: isOn ? "heart.fill" : "heart")
                .foregroundStyle(isOn ? AnyShapeStyle(.red) : AnyShapeStyle(.primary))
        }
        .help(isOn ? "Remove from favourites" : "Add to favourites")
    }
}

/// The big play button on an album or artist page, sat beside the title.
struct PlayableHeaderButton: View {
    let playable: Playable

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @State private var loading = false

    var body: some View {
        Button {
            guard !loading else { return }
            loading = true
            let engine = library.engine
            let playable = self.playable
            Task {
                let ids = await playable.trackIds(using: engine)
                loading = false
                player.playNow(trackIds: ids)
                nav.showQueueWhenReady(watching: player)
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


/// "Play Next" and "Queue", side by side under a page title.
///
/// One component rather than a pair written out per page: the album page and
/// the artist page offer the same two verbs, and hand-rolling them twice is how
/// the artist page ended up without them.
struct QueueButtons: View {
    let playable: Playable?

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var working = false

    var body: some View {
        HStack(spacing: 10) {
            Button { act { player.playNext(trackIds: $0) } } label: {
                Label("Play Next", systemImage: "text.line.first.and.arrowtriangle.forward")
            }
            Button { act { player.enqueue(trackIds: $0) } } label: {
                Label("Queue", systemImage: "text.append")
            }
            if working {
                ProgressView().controlSize(.small)
            }
        }
        .disabled(playable == nil || working)
    }

    /// An artist is thousands of tracks and resolving them is a database read,
    /// so it happens off the main actor.
    private func act(_ body: @escaping @MainActor ([Int64]) -> Void) {
        guard let playable else { return }
        working = true
        let engine = library.engine
        Task {
            let ids = await playable.trackIds(using: engine)
            working = false
            guard !ids.isEmpty else { return }
            body(ids)
        }
    }
}


/// "Add to Playlist" — every playlist, plus a new one.
///
/// Its own view rather than lines inside each menu: the same submenu belongs on
/// a library row, a queue row and a playlist row, and the three would otherwise
/// drift. `resolve` defers working out the tracks until something is chosen —
/// an artist is thousands of them, and resolving to *draw* a menu is what froze
/// the window when the queue's context menus did it.
struct AddToPlaylistMenu: View {
    /// Hands the chosen action the track ids, off the main actor.
    let resolve: (@escaping @MainActor ([Int64]) -> Void) -> Void

    @Environment(PlaylistsModel.self) private var playlists
    @Environment(Navigator.self) private var nav
    @State private var naming = false
    @State private var newName = ""

    var body: some View {
        Menu("Add to Playlist") {
            Button("New Playlist…") { naming = true }
            if !playlists.playlists.isEmpty {
                Divider()
                ForEach(playlists.playlists, id: \.id) { playlist in
                    Button(playlist.name) {
                        resolve { playlists.add(trackIds: $0, to: playlist.id) }
                    }
                }
            }
        }
        .alert("New Playlist", isPresented: $naming) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) { newName = "" }
            Button("Create") {
                let name = newName
                newName = ""
                resolve { ids in
                    Task {
                        guard let created = await playlists.create(named: name, trackIds: ids)
                        else { return }
                        nav.open(playlist: created.id)
                    }
                }
            }
        }
    }
}
