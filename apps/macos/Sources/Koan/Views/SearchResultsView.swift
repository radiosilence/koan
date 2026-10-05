import KoanFFI
import SwiftUI

/// Everything matching the query, in sections. Selecting a result takes you to
/// where it lives in the library rather than playing it — you play from the
/// album or artist page, the same way you would if you'd browsed there.
struct SearchResultsView: View {
    @Environment(SearchModel.self) private var search
    @Environment(Navigator.self) private var nav
    @Environment(UIState.self) private var ui
    @Environment(\.onStage) private var onStage
    @Environment(LibraryModel.self) private var library
    #if os(macOS)
    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror
    @Environment(CoverArtCache.self) private var art
    @Environment(PlayingLevels.self) private var levels
    @Environment(TransferMeter.self) private var meter
    @Environment(\.roomTint) private var tint
    @AppStorage("graphics") private var graphics = Graphics.full
    #endif

    private let columns = [GridItem(.adaptive(minimum: 140, maximum: 190), spacing: 16)]

    var body: some View {
        page
        .navigationTitle(search.hasQuery ? "Results for “\(search.query)”" : "Search")
        // The album browser's pick, over every kind of result. It survives a
        // new query, so a pick can gather from several searches; it ends with
        // the page.
        .onChange(of: ui.selectAllToken) { _, _ in
            guard onStage else { return }
            search.selection.selectAll()
        }
        .onChange(of: ui.clearSelectionToken) { _, _ in search.selection.end() }
        .onChange(of: onStage) { _, now in if !now { search.selection.end() } }
        .onDisappear { search.selection.end() }
    }

    @ViewBuilder private var page: some View {
        #if os(macOS)
        if search.hasQuery, !search.isEmpty || search.isSearching {
            results
        } else {
            ScrollView { empty }
        }
        #else
        ScrollView {
            if !search.hasQuery || (search.isEmpty && !search.isSearching) {
                empty
            } else {
                VStack(alignment: .leading, spacing: 26) {
                    if !search.artists.isEmpty { artistSection }
                    if !search.albums.isEmpty { albumSection }
                    if !search.tracks.isEmpty { trackSection }
                }
                .padding(22)
                .modifier(SelectionDrag(selection: search.selection))
            }
        }
        #endif
    }

    @ViewBuilder private var empty: some View {
        if !search.hasQuery {
            EmptyState(icon: "magnifyingglass", title: "Search your library")
                .frame(maxWidth: .infinity, minHeight: 320)
        } else {
            EmptyState(
                icon: "magnifyingglass",
                title: "Nothing found",
                detail: "No artists, albums or tracks match “\(search.query)”."
            )
            .frame(maxWidth: .infinity, minHeight: 320)
        }
    }

    #if os(macOS)
    /// A `MixedCollection` — see there for why the Mac's page is AppKit.
    private var results: some View {
        let pick = search.selection
        let picked = Set(pick.picked.map(\.key))
        let lines = search.tracks.map { TrackLine(id: $0.id, kind: .track($0)) }
        let queued = mirror.queuedByTrack
        let key: [AnyHashable] = [
            AnyHashable(picked), AnyHashable(pick.isActive), AnyHashable(tint),
            AnyHashable(library.favouriteAlbumIds),
            AnyHashable(queued.map { "\($0.key):\($0.value.status)" }.sorted()),
        ]
        let library = library
        let nav = nav
        let player = player
        return SafeAreaReader { insets in
            MixedCollection(
                artists: search.artists,
                albums: search.albums,
                tracks: lines,
                tileContext: AlbumTile.Context(
                    art: art,
                    selection: pick,
                    picked: picked,
                    selecting: pick.isActive,
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
                    menu: { _ in NSMenu() }
                ),
                trackContext: TrackTableRow.Context(
                    showsAlbum: true,
                    columns: [.availability],
                    leadWidth: 18,
                    picking: pick.isActive,
                    picked: picked,
                    currentTrackId: player.currentTrackId,
                    isPlaying: player.isPlaying,
                    barsLive: false,
                    tint: NSColor(tint),
                    favourites: [],
                    queued: queued,
                    meter: meter,
                    art: art,
                    levels: levels,
                    play: { line in if let track = line.track { player.playNow(trackIds: [track.id]) } },
                    openArtist: { nav.open(artist: $0) },
                    openAlbum: { nav.open(album: $0) },
                    toggleFavourite: { _ in }
                ),
                contextKey: AnyHashable(key),
                selection: .constant([]),
                albumMenu: { album, environment in
                    hostedMenu(PlayableMenu(playable: .album(album)), environment: environment)
                },
                artistMenu: { artist, environment in
                    hostedMenu(PlayableMenu(playable: .artist(id: artist.id, name: artist.name)), environment: environment)
                },
                trackMenu: { ids, environment in
                    guard let track = search.tracks.first(where: { ids.contains($0.id) }) else { return nil }
                    return hostedMenu(PlayableMenu(playable: .track(track)), environment: environment)
                },
                openArtist: { nav.open(artist: $0) },
                primaryAction: { _ in },
                tracksSelect: false,
                openTrack: { track in
                    if let albumId = track.albumId { nav.open(album: albumId, highlighting: track.id) }
                },
                pick: pick,
                counts: true,
                totals: search.totals,
                seeAll: { nav.show(library.seeAll($0, of: .search(query: search.query))) },
                insets: insets
            )
        }
    }
    #endif

    private var artistSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeading("Artists", count: search.artists.count, total: search.totals?.artists) {
                nav.show(library.seeAll(.artists, of: .search(query: search.query)))
            }
            FlowLayout(spacing: 8) {
                ForEach(search.artists, id: \.id) { artist in
                    ArtistPill(name: artist.name, artistId: artist.id, selection: search.selection)
                }
            }
        }
    }

    private var albumSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeading("Albums", count: search.albums.count, total: search.totals?.albums) {
                nav.show(library.seeAll(.albums, of: .search(query: search.query)))
            }
            LazyVGrid(columns: columns, spacing: 18) {
                ForEach(search.albums, id: \.id) { album in
                    AlbumGridCell(album: album, selection: search.selection)
                        .contentShape(Rectangle())
                        .onTapGesture { nav.open(album: album.id) }
                }
            }
        }
    }

    private var trackSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionHeading("Tracks", count: search.tracks.count, total: search.totals?.tracks) {
                nav.show(library.seeAll(.tracks, of: .search(query: search.query)))
            }
            VStack(spacing: 0) {
                ForEach(search.tracks, id: \.id) { track in
                    SearchTrackRow(track: track, selection: search.selection)
                }
            }
        }
    }
}

private struct SectionHeading: View {
    let title: String
    let count: Int
    /// How many the library has, when it is more than the results show.
    let total: UInt64?
    let seeAll: () -> Void

    init(_ title: String, count: Int, total: UInt64?, seeAll: @escaping () -> Void) {
        self.title = title
        self.count = count
        self.total = total
        self.seeAll = seeAll
    }

    var body: some View {
        HStack(spacing: 7) {
            Text(title)
                .font(.title3.weight(.semibold))
            Text("\(count)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
            Spacer()
            if let total, total > UInt64(count) {
                Button("See all (\(total))", action: seeAll)
                    .buttonStyle(.borderless)
                    .font(.subheadline)
            }
        }
    }
}

/// A track result points at its album — that's "the place in the library" the
/// track lives, and where you'd play it from.
///
/// Behaves like any other row: click to go where it lives, drag it to enqueue,
/// right-click for the same menu the tiles and pills have. Not a `Button`,
/// which claims the press and leaves the row impossible to drag or
/// right-click.
private struct SearchTrackRow: View {
    let track: Track
    let selection: PlayableSelection

    @Environment(EngineMirror.self) private var mirror
    @Environment(Navigator.self) private var nav
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            if selection.isActive {
                SelectionTick(key: Playable.track(track).key, selection: selection)
            }

            // The cover is what you recognise a track by, and a results
            // list is exactly where you are trying to recognise something.
            TrackSleeve(albumId: track.albumId)
                .frame(width: 40, height: 40)

            VStack(alignment: .leading, spacing: 1) {
                Text(track.title).lineLimit(1)
                Text("\(track.artistName) — \(track.albumTitle)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                SourceBadges(track: track, queued: mirror.queuedByTrack[track.id])
            }

            Spacer(minLength: 8)

            if track.albumId != nil && hovering && !selection.isActive {
                Text("Go to album")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Text(Format.duration(track.durationMs))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background {
            RoundedRectangle(cornerRadius: 6)
                .fill(hovering ? AnyShapeStyle(.quaternary.opacity(0.5)) : AnyShapeStyle(.clear))
        }
        .rowBehaviour()
        .modifier(SelectableDrag(playable: .track(track), inContainer: true))
        .onTapGesture {
            if selection.take(.track(track)) { return }
            guard let albumId = track.albumId else { return }
            nav.open(album: albumId, highlighting: track.id)
        }
        .contextMenu { PlayableMenu(playable: .track(track)) }
        .onHover { hovering = $0 }
    }
}
