import KoanFFI
import SwiftUI

/// A page of artists, records and tracks that answer one question: what you
/// favourited, what you played lately. Artists as pills, records as tiles,
/// tracks as a working list below them.
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
    let artists: [Artist]
    let albums: [Album]
    let tracks: [Track]
    /// What an empty page says.
    let empty: EmptyShelf
    /// How much of each record is on this device, where the page shows it.
    var fractions: [Int64: Double] = [:]

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
    private static let tileSpacing: CGFloat = 16
    /// What an inset list keeps clear at each side.
    private static let listInset: CGFloat = 20

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if artists.isEmpty && albums.isEmpty && tracks.isEmpty {
                EmptyState(
                    icon: empty.icon,
                    title: library.filter.isEmpty ? empty.title : "No matches",
                    detail: library.filter.isEmpty ? empty.detail : nil
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                #if os(macOS)
                collection
                #else
                list
                #endif
            }
        }
    }

    #if os(macOS)
    /// A `MixedCollection` — see there for why the Mac's page is AppKit.
    private var collection: some View {
        let lines = tracks.enumerated().map { TrackLine(id: $1.id, kind: .track($1), lead: "\($0 + 1)", position: $0) }
        let queued = mirror.queuedByTrack
        let current = player.currentTrackId
        let playing = player.isPlaying
        let live = onStage && !reduceMotion && graphics.animatesIndicators
        let key: [AnyHashable] = [
            AnyHashable(current), AnyHashable(playing), AnyHashable(live), AnyHashable(tint),
            AnyHashable(library.favouriteTrackIds), AnyHashable(library.favouriteAlbumIds),
            AnyHashable(queued.map { "\($0.key):\($0.value.status)" }.sorted()),
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
                    fractions: fractions
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
                selectAllToken: ui.selectAllToken,
                insets: insets
            )
        }
        .clearsSelection($selection)
    }
    #endif

    private var list: some View {
                List(selection: $selection) {
                    if !artists.isEmpty { artistSection }
                    if !albums.isEmpty { albumSection }
                    if !tracks.isEmpty { trackSection }
                }
                .listStyle(.inset)
                .washedGround()
                .clearsSelection($selection)
                .contextMenu(forSelectionType: Int64.self) { ids in
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
            Text(title)
                .font(.title2.weight(.semibold))
            Text(summary)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Only the kinds you have, so a tracks-only library reads as a count of
    /// tracks.
    private var summary: String {
        var parts: [String] = []
        if !artists.isEmpty { parts.append(Format.count(Int64(artists.count), "artist")) }
        if !albums.isEmpty { parts.append(Format.count(Int64(albums.count), "album")) }
        if !tracks.isEmpty { parts.append(Format.count(Int64(tracks.count), "track")) }
        return parts.joined(separator: " · ")
    }

    // MARK: - Sections

    private var artistSection: some View {
        Section("Artists") {
            FlowLayout(spacing: 8) {
                ForEach(artists, id: \.id) { artist in
                    ArtistPill(name: artist.name, artistId: artist.id)
                }
            }
            .padding(.vertical, 4)
            .selectionDisabled()
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
        Section("Albums") {
            ForEach(albumRows, id: \.first!.id) { row in
                HStack(alignment: .top, spacing: Self.tileSpacing) {
                    ForEach(row, id: \.id) { album in
                        AlbumGridCell(album: album)
                            .overlay(alignment: .top) {
                                if let fraction = fractions[album.id] {
                                    DownloadedBar(fraction: fraction)
                                }
                            }
                            .frame(maxWidth: Self.tileMax)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.vertical, 6)
                .selectionDisabled()
            }
        }
    }

    /// How many tiles fit across, the way an adaptive grid would decide it.
    private var albumColumns: Int {
        let usable = width - Self.listInset * 2 + Self.tileSpacing
        return max(1, Int(usable / (Self.tileMin + Self.tileSpacing)))
    }

    private var albumRows: [[Album]] {
        let per = albumColumns
        return stride(from: 0, to: albums.count, by: per).map { start in
            Array(albums[start..<min(start + per, albums.count)])
        }
    }

    private var trackSection: some View {
        Section("Tracks") {
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
                .primaryTap { play([track.id]) }
            }
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

/// How much of a record is on this device, along the foot of its sleeve.
private struct DownloadedBar: View {
    let fraction: Double

    var body: some View {
        GeometryReader { geo in
            let side = geo.size.width
            Capsule()
                .fill(.black.opacity(0.35))
                .overlay(alignment: .leading) {
                    Capsule()
                        .fill(.tint)
                        .frame(width: (side - 16 - 34) * min(max(fraction, 0), 1))
                }
                .frame(width: max(side - 16 - 34, 0), height: 3)
                .offset(x: 8, y: side - 8 - 3)
        }
        .aspectRatio(1, contentMode: .fit)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
