import KoanFFI
import SwiftUI

/// A page of artists, records and tracks that answer one question: what you
/// favourited, what you played lately. Artists as pills, records as tiles,
/// tracks as a working list below them.
///
/// The first few of each, as `koan_core::shelves` cuts them, with how many
/// there are. Each section's heading opens that kind's browser filtered to
/// the shelf: the listing the preview is the head of, with the count the
/// heading gave.
///
/// Sections rather than a type picker, for the same reason search results are
/// sections: they are all answers to one question, and a mode you have to
/// remember you are in is a worse way to find a record.
///
/// One `List` rather than search's `ScrollView`, because the tracks here are a
/// working list: range-select, Return to play, a menu on the selection. The
/// artists and records ride above them as rows that cannot be selected.
struct ShelfView: View {
    let title: String
    let shelf: ShelfKind
    let summary: ShelfSummary?
    /// What an empty page says.
    let empty: EmptyShelf

    private var artists: [Artist] { summary?.artists ?? [] }
    private var albums: [Album] { summary?.albums ?? [] }
    private var tracks: [Track] { summary?.tracks ?? [] }

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library

    @State private var selection: Set<Int64> = []
    #if os(macOS)
    @Environment(EngineMirror.self) private var mirror
    @Environment(CoverArtCache.self) private var art
    @Environment(PlayingLevels.self) private var levels
    @Environment(TransferMeter.self) private var meter
    @Environment(UIState.self) private var ui
    @Environment(\.roomTint) private var tint
    @Environment(\.onStage) private var onStage
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AppStorage("graphics") private var graphics = Graphics.full
    #endif
    /// The list's width, for how many records fit across a row.
    @State private var width: CGFloat = 0

    /// The same tile the grids use, at the same sizes.
    private static let tileMin: CGFloat = 140
    private static let tileMax: CGFloat = 190
    private static let tileSpacing = KoanTheme.Space.l
    /// What an inset list keeps clear at each side.
    private static let listInset: CGFloat = 20

    var body: some View {
        VStack(spacing: 0) {
            // On iOS the navigation bar already says what the page is; the
            // counts go under its title.
            #if os(macOS)
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)
            #endif

            if artists.isEmpty && albums.isEmpty && tracks.isEmpty {
                EmptyState(icon: empty.icon, title: empty.title, detail: empty.detail)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                #if os(macOS)
                collection
                #else
                list
                #endif
            }
        }
        #if os(iOS)
        .navigationSubtitle(counts)
        #endif
    }

    #if os(macOS)
    /// A `MixedCollection` — see there for why the Mac's page is AppKit.
    private var collection: some View {
        let lines = tracks.enumerated().map { TrackLine(id: $1.id, kind: .track($1), lead: "\($0 + 1)", position: $0) }
        let queued = mirror.queuedByTrack
        let current = player.currentTrackId
        let playing = player.isPlaying
        let live = onStage && !reduceMotion
        let key: [AnyHashable] = [
            AnyHashable(current), AnyHashable(playing), AnyHashable(live), AnyHashable(tint),
            AnyHashable(library.favouriteTrackIds), AnyHashable(library.favouriteAlbumIds),
            AnyHashable(queued.map { "\($0.key):\($0.value.status)" }.sorted()),
            AnyHashable(mirror.arrivingByAlbum),
        ]
        let library = library
        let nav = nav
        let player = player
        return SafeAreaReader { insets in
            MixedCollection(
                artists: artists,
                albums: albums,
                tracks: lines,
                tileContext: AlbumTile.Context(
                    art: art,
                    selection: nil,
                    picked: [],
                    selecting: false,
                    favourites: library.favouriteAlbumIds,
                    tint: NSColor(tint),
                    usesGlass: graphics.usesGlass,
                    actions: AlbumTile.Actions(
                        open: { nav.open(album: $0) },
                        openArtist: { nav.open(artist: $0) },
                        play: { id in
                            nav.open(album: id)
                            let engine = library.engine
                            let ids = await Task.detached { (try? await engine.trackIds(albumId: id, artistId: nil)) ?? [] }.value
                            player.playNow(trackIds: ids)
                        },
                        toggleFavourite: { library.toggleFavourite(album: $0) }
                    ),
                    menu: { _ in NSMenu() },
                    arriving: mirror.arrivingByAlbum,
                    meter: meter
                ),
                trackContext: TrackTableRow.Context(
                    showsAlbum: true,
                    leadWidth: TrackTableRow.leadWidth(for: tracks.count),
                    currentTrackId: current,
                    isPlaying: playing,
                    barsLive: live,
                    tint: NSColor(tint),
                    favourites: library.favouriteTrackIds,
                    queued: queued,
                    meter: meter,
                    art: art,
                    levels: levels,
                    play: { line in play([line.id]) },
                    openArtist: { nav.open(artist: $0) },
                    openAlbum: { nav.open(album: $0) },
                    toggleFavourite: { library.toggleFavourite(track: $0) }
                ),
                contextKey: AnyHashable(key),
                selection: $selection,
                albumMenu: { album, environment in
                    hostedMenu(PlayableMenu(playable: .album(album)), environment: environment)
                },
                artistMenu: { artist, environment in
                    hostedMenu(PlayableMenu(playable: .artist(id: artist.id, name: artist.name)), environment: environment)
                },
                trackMenu: { ids, environment in hostedMenu(menu(for: ids), environment: environment) },
                openArtist: { nav.open(artist: $0) },
                primaryAction: play,
                totals: summary.map(ShelfTotals.init),
                openSection: open,
                selectAllToken: ui.selectAllToken,
                insets: insets
            )
        }
        .clearsSelection($selection)
    }
    #endif

    private var list: some View {
                List(selection: $selection) {
                    Group {
                        if !artists.isEmpty { artistSection }
                        if !albums.isEmpty { albumSection }
                        if !tracks.isEmpty { trackSection }
                    }
                    .washedRow()
                }
                .insetList()
                .washedGround()
                .clearsSelection($selection)
                .selectionMenu(for: Int64.self) { ids in
                    menu(for: ids)
                } primaryAction: { ids in
                    play(ids)
                }
                .onKeyPress(.return) {
                    play(selection)
                    return .handled
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 1) {
            // A television's navigation title already names the page, above.
            #if !os(tvOS)
            Text(title).koanCase()
                .font(.role(.titleSmall, system: .title2.weight(.semibold)))
            #endif
            Text(counts)
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Only the kinds you have, so a tracks-only library reads as a count of
    /// tracks. The whole shelf's counts, not the preview's.
    private var counts: String {
        guard let summary else { return "" }
        var parts: [String] = []
        if summary.artistTotal > 0 { parts.append(Format.count(Int64(summary.artistTotal), "artist")) }
        if summary.albumTotal > 0 { parts.append(Format.count(Int64(summary.albumTotal), "album")) }
        if summary.trackTotal > 0 { parts.append(Format.count(Int64(summary.trackTotal), "track")) }
        return parts.joined(separator: " · ")
    }

    /// Open the browser for `list`, filtered to this shelf.
    private func open(_ list: LibraryModel.ShelfList) {
        nav.show(library.browse(list, of: shelf))
    }

    /// A section's heading: its name, how many the shelf has in all, and a
    /// chevron, the whole of it opening the browser filtered to the shelf.
    /// The preview below may show fewer.
    private func sectionHead(_ title: String, total: UInt64, list: LibraryModel.ShelfList) -> some View {
        Button { open(list) } label: {
            HStack(spacing: 6) {
                Text(title).koanCase()
                Text("\(total)")
                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                    .monospacedDigit()
                Image(systemName: "chevron.right")
                    .font(.role(.fine, system: .caption.weight(.semibold)))
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        // A television's focus platter is white: `TelevisionRow` draws the
        // heading as on a light screen there, where a plain button keeps the
        // header's light text on it.
        #if os(tvOS)
        .buttonStyle(TelevisionRow())
        #else
        .buttonStyle(.plain)
        #endif
        .accessibilityIdentifier("heading-\(list)")
    }

    // MARK: - Sections

    private var artistSection: some View {
        Section {
            FlowLayout(spacing: 8) {
                ForEach(artists, id: \.id) { artist in
                    ArtistPill(name: artist.name, artistId: artist.id)
                }
            }
            .padding(.vertical, 4)
            .selectionDisabled()
        } header: {
            sectionHead("Artists", total: summary?.artistTotal ?? 0, list: .artists)
        }
    }

    /// The records, a row of tiles per List row.
    ///
    /// Not a `LazyVGrid`. A grid inside a List row is one cell, and a cell is
    /// laid out whole, so every record on the page would be built
    /// and asked for its sleeve the moment the page opened — hundreds of
    /// fetches at once, and none of them cancelled by scrolling away, because
    /// nothing would ever scroll off. Rows of the List are what the List recycles, so the
    /// grid is cut into them.
    private var albumSection: some View {
        Section {
            ForEach(albumRows, id: \.first!.id) { row in
                // Across the whole row, so the grid keeps the margins its
                // headings do; a short last row keeps the others' columns.
                HStack(alignment: .top, spacing: Self.tileSpacing) {
                    ForEach(row, id: \.id) { album in
                        AlbumGridCell(album: album)
                            .frame(maxWidth: .infinity)
                    }
                    ForEach(row.count..<max(row.count, albumColumns), id: \.self) { _ in
                        Color.clear.frame(maxWidth: .infinity, maxHeight: 0)
                    }
                }
                .padding(.vertical, 6)
                .selectionDisabled()
            }
        } header: {
            sectionHead("Albums", total: summary?.albumTotal ?? 0, list: .albums)
        }
    }

    /// How many tiles go across: the fewest that keep each within the
    /// grids' largest tile, as long as each stays at least their smallest.
    private var albumColumns: Int {
        let usable = width - Self.listInset * 2 + Self.tileSpacing
        let most = max(1, Int(usable / (Self.tileMin + Self.tileSpacing)))
        return min(most, max(1, Int((usable / (Self.tileMax + Self.tileSpacing)).rounded(.up))))
    }

    private var albumRows: [[Album]] {
        let per = albumColumns
        return stride(from: 0, to: albums.count, by: per).map { start in
            Array(albums[start..<min(start + per, albums.count)])
        }
    }

    private var trackSection: some View {
        Section {
            // Once per pass, not once per row — see `TrackListView`.
            let allTrackIds = tracks.map(\.id)
            ForEach(Array(tracks.enumerated()), id: \.element.id) { index, track in
                TrackRow(
                    track: track,
                    position: index + 1,
                    // Gathered from all over, so a row carries its own sleeve
                    // and says which record it came from.
                    showsAlbum: true,
                    allTrackIds: allTrackIds
                )
                .rowBehaviour(playable: .track(track))
                .primaryTap { play([track.id]) } menu: { menu(for: [track.id]) }
                .accessibilityIdentifier("track-\(track.id)")
            }
        } header: {
            sectionHead("Tracks", total: summary?.trackTotal ?? 0, list: .tracks)
        }
    }

    // MARK: - Actions

    /// Plays from the first of `ids`, keeping the rest of the list behind it.
    private func play(_ ids: Set<Int64>) {
        guard let index = tracks.firstIndex(where: { ids.contains($0.id) }) else { return }
        player.playNow(trackIds: tracks.map(\.id), startingAt: index)
        nav.showQueueWhenReady(watching: player)
    }

    @ViewBuilder
    private func menu(for ids: Set<Int64>) -> some View {
        let chosen = tracks.filter { ids.contains($0.id) }
        if chosen.count == 1, let track = chosen.first {
            PlayableMenu(playable: .track(track))
        } else if !chosen.isEmpty {
            Button("Play") {
                player.playNow(trackIds: chosen.map(\.id))
                nav.showQueueWhenReady(watching: player)
            }
            Button("Play Next") { player.playNext(trackIds: chosen.map(\.id)) }
            Button("Add to Queue") { player.enqueue(trackIds: chosen.map(\.id)) }
        }
    }
}

/// What a shelf with nothing on it says, before any filter narrows it.
struct EmptyShelf {
    let icon: String
    let title: String
    let detail: String?
}
