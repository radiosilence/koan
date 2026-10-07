import KoanFFI
import SwiftUI

struct ArtistBrowser: View {
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    /// Without a selection binding a List row has nothing to do with a click.
    @State private var selection: Set<Int64> = []
    #if os(iOS)
    @State private var editMode: EditMode = .inactive
    #endif
    #if os(macOS)
    @Environment(PlayerModel.self) private var player
    @Environment(UIState.self) private var ui
    @Environment(\.roomTint) private var tint
    #endif

    var body: some View {
        Group {
            #if os(macOS)
            table
            #else
            list
            #endif
        }
        // Once narrowed, how many: the count a shelf's heading gave.
        .pageSubtitle(library.isNarrowed ? Format.count(Int64(library.visibleArtists.count), "artist") : "")
    }

    #if os(macOS)
    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private var table: some View {
        SafeAreaReader { insets in
            KoanTable(
                items: library.visibleArtists,
                id: \.id,
                context: ArtistTableRow.Context(
                    favourites: library.favouriteArtistIds,
                    tint: NSColor(tint),
                    play: { artist in
                        let engine = library.engine
                        await player.playNow(resolving: artist.name) {
                            await Playable.artist(id: artist.id, name: artist.name).trackIds(using: engine)
                        }.value
                    },
                    open: { nav.open(artist: $0) },
                    toggleFavourite: { library.toggleFavourite(artist: $0) }
                ),
                contextKey: AnyHashable([AnyHashable(library.favouriteArtistIds), AnyHashable(tint)]),
                selection: $selection,
                make: ArtistTableRow.init,
                menu: { ids, environment in
                    // A set has no first; with several picked, no one artist is meant.
                    guard ids.count == 1, let id = ids.first,
                          let artist = library.visibleArtists.first(where: { $0.id == id })
                    else { return nil }
                    return hostedMenu(PlayableMenu(playable: .artist(id: artist.id, name: artist.name)), environment: environment)
                },
                primaryAction: { ids in
                    if ids.count == 1, let id = ids.first { nav.open(artist: id) }
                },
                drag: { ids in
                    library.visibleArtists.filter { ids.contains($0.id) }
                        .map { PlayableTransfer(.artist(id: $0.id, name: $0.name)) }
                },
                selectAllToken: ui.selectAllToken,
                offset: library.artistsOffset,
                noteOffset: { library.artistsOffset = $0 },
                rewinds: nav.rewinds[.artists] ?? 0,
                insets: insets
            )
        }
        .clearsSelection($selection)
        .overlay {
            if library.visibleArtists.isEmpty {
                EmptyState(icon: "music.mic", title: library.isNarrowed ? "Nothing matches" : "No artists yet")
            }
        }
    }
    #endif

    private var list: some View {
        ScrollViewReader { proxy in
        List(library.visibleArtists, id: \.id, selection: $selection) { artist in
            ArtistRow(artist: artist)
                .primaryTap { nav.open(artist: artist.id) } menu: {
                    PlayableMenu(playable: .artist(id: artist.id, name: artist.name))
                }
                .onAppear { library.artistsShown.insert(artist.id) }
                .onDisappear { library.artistsShown.remove(artist.id) }
                .washedRow()
        }
        // Rebuilt on each visit rather than kept mounted behind other pages
        // (see `StageView`). A `List` takes no scroll position, but it does go
        // to a row it is asked for, so the top row is noted on the way out and
        // asked for on the way back.
        .onAppear {
            guard let top = library.artistsTop else { return }
            proxy.scrollTo(top, anchor: .top)
            // The list also builds a few rows above the ones on screen. Seen
            // once here, where the true top is known, so leaving does not
            // remember a place a little above where you were each time.
            Task {
                try? await Task.sleep(for: .milliseconds(300))
                if let wanted = artistIndex(top), let built = topShownIndex {
                    library.artistsOverscan = max(0, wanted - built)
                }
            }
        }
        .onDisappear { library.artistsTop = topShownArtist }
        .onChange(of: nav.rewinds[.artists]) {
            if let first = library.visibleArtists.first { proxy.scrollTo(first.id, anchor: .top) }
        }
        }
        #if os(iOS)
        .listSelectMode($editMode, selection: $selection) { ids in
            let engine = library.engine
            return SelectionBar.Actions(
                favourites: ids.map { Playable.Key(kind: .artist, id: $0) },
                tracks: {
                    var tracks: [Int64] = []
                    for id in ids {
                        tracks += (try? await engine.trackIds(albumId: nil, artistId: id)) ?? []
                    }
                    return tracks
                }
            )
        }
        #endif
        .clearsSelection($selection)
        .washedGround()
        .selectionMenu(for: Int64.self) { ids in
            // A set has no first; with several picked, no one artist is meant.
            if ids.count == 1, let id = ids.first,
               let artist = library.visibleArtists.first(where: { $0.id == id }) {
                PlayableMenu(playable: .artist(id: artist.id, name: artist.name))
            }
        } primaryAction: { ids in
            if ids.count == 1, let id = ids.first { nav.open(artist: id) }
        }
        .overlay {
            if library.visibleArtists.isEmpty {
                EmptyState(icon: "music.mic", title: library.isNarrowed ? "Nothing matches" : "No artists yet")
            }
        }
    }

    /// The first row on screen: the highest row built, less the rows the list
    /// builds above what it shows.
    private var topShownArtist: Int64? {
        guard let built = topShownIndex else { return nil }
        let artists = library.visibleArtists
        let index = built == 0 ? 0 : min(built + library.artistsOverscan, artists.count - 1)
        return artists[index].id
    }

    private var topShownIndex: Int? {
        let shown = library.artistsShown
        guard !shown.isEmpty else { return nil }
        return library.visibleArtists.firstIndex { shown.contains($0.id) }
    }

    private func artistIndex(_ id: Int64) -> Int? {
        library.visibleArtists.firstIndex { $0.id == id }
    }
}

/// One artist.
///
/// Its own view so that hovering it invalidates one row. With the hover state
/// on the browser, every pointer move across the list would rebuild all of it —
/// thousands of rows diffed to light up a play button.
private struct ArtistRow: View {
    let artist: Artist

    @State private var hovered = false

    var body: some View {
        HStack(spacing: 10) {
            Group {
                if hovered {
                    RowPlayButton(playable: playable, visible: true)
                } else {
                    Image(systemName: "music.mic")
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                }
            }
            .frame(width: 18, height: 18)
            // The name is the way in — a link, so a single click opens the
            // artist while the rest of the row selects.
            #if os(iOS) || os(tvOS)
            // Too narrow for count columns: they would take the name's room.
            VStack(alignment: .leading, spacing: 2) {
                LinkText(
                    text: artist.name,
                    target: .artist(artist.id),
                    font: .role(.body, system: .body),
                    prominent: true
                )
                Text(
                    "\(Format.count(artist.albumCount, "album")) · \(Format.count(artist.trackCount, "track"))"
                )
                .font(.role(.fine, system: .caption.monospacedDigit()))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            }
            Spacer(minLength: 0)
            ArtistHeart(artistId: artist.id, showing: hovered, size: .caption)
                .frame(width: 16)
            #else
            LinkText(
                text: artist.name,
                target: .artist(artist.id),
                font: .role(.body, system: .body),
                prominent: true
            )
            ArtistHeart(artistId: artist.id, showing: hovered, size: .caption)
                .frame(width: 16)
            Spacer(minLength: 12)
            Text(Format.count(artist.albumCount, "album"))
                .font(.role(.fine, system: .caption.monospacedDigit()))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                .frame(width: 78, alignment: .trailing)
            Text(Format.count(artist.trackCount, "track"))
                .font(.role(.fine, system: .caption.monospacedDigit()))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                .frame(width: 78, alignment: .trailing)
            #endif
        }
        .pointerHover { hovered = $0 }
        #if os(iOS) || os(tvOS)
        .frame(minHeight: RowMetrics.line)
        #else
        .frame(height: RowMetrics.line)
        #endif
        .rowBehaviour(playable: playable)
    }

    private var playable: Playable { .artist(id: artist.id, name: artist.name) }
}

/// An artist's records as a grid, since that's how people think about a
/// discography — the flat track list is a click away on each album.
struct ArtistDetailView: View {
    let artistId: Int64

    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @Environment(UIState.self) private var ui
    /// Off stage while a page is pushed over it on a phone — see `StageView`.
    @Environment(\.onStage) private var onStage

    /// Whatever the navigator loaded before it brought us here, so the first
    /// body evaluation already has the whole page. Guarded on the id because
    /// history can move faster than a read.
    private var record: LibraryModel.ArtistRecord? {
        let held = library.detailArtist
        return held?.artistId == artistId ? held : nil
    }

    private var artist: Artist? { record?.artist }
    private var albums: [Album] { record?.albums ?? [] }
    private var info: ArtistInfo? { record?.info }

    private let columns = GridItem.tiles(minimum: 150, maximum: 210, spacing: 18)

    var body: some View {
        if let record, record.albums.isEmpty, !record.appearances.isEmpty {
            // Credited only on other people's records: the tracks, each with
            // the album it is on, rather than an empty grid.
            TrackListView(
                title: artist?.name ?? "",
                subtitle: "Appears on \(Set(record.appearances.map(\.albumId)).count) "
                    + (Set(record.appearances.map(\.albumId)).count == 1 ? "album" : "albums"),
                tracks: record.appearances,
                mixedAlbums: true
            )
            .reloading(on: artistId) { await library.prepare(artist: artistId) }
        } else {
            discography
        }
    }

    private var discography: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                // The play button reads as part of the title, so it sits on the
                // title's line. Everything below is full width rather than
                // indented into a column beside it.
                // Top-aligned, as a record's header is: the photo's top edge
                // meets the name's first line.
                HStack(alignment: .top, spacing: 20) {
                    if info?.hasImage == true {
                        AlbumArtwork(source: .artist(artistId), size: .tile, cornerRadius: KoanTheme.radius(56))
                            .frame(width: 112, height: 112)
                            .transition(.opacity)
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(alignment: .firstTextBaseline, spacing: 14) {
                            #if !os(tvOS)
                            if let artist {
                                PlayableHeaderButton(
                                    playable: .artist(id: artist.id, name: artist.name)
                                )
                                .alignmentGuide(.firstTextBaseline) { $0[.bottom] * 0.78 }
                            }
                            #endif
                            Text(artist?.name ?? "Artist")
                                // The album page's title size, on each platform.
                                #if os(tvOS)
                                .font(.role(.display, system: .system(size: 48, weight: .semibold)))
                                #else
                                .font(.role(.title, system: .system(size: 26, weight: .semibold)))
                                #endif
                        }
                        Text(Format.count(Int64(albums.count), "album"))
                            .font(.role(.control, system: .callout))
                            .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        if let artist {
                            let playable = Playable.artist(id: artist.id, name: artist.name)
                            HeaderActions(playable: playable, shuffle: shufflePlay)
                                .padding(.top, 4)
                        }
                    }
                }
                .animation(.easeOut(duration: 0.2), value: info?.hasImage)

                LazyVGrid(columns: columns, spacing: 22) {
                    ForEach(albums, id: \.id) { album in
                        AlbumGridCell(album: album, showArtist: false, selection: library.artistSelection)
                    }
                }
                .modifier(SelectionDrag(selection: library.artistSelection))

                if let info, let bio = info.bio {
                    Divider()
                    ArtistBio(bio: bio, source: info.bioUrl, imageCredit: info.imageCredit)
                        .transition(.opacity)
                } else if record?.infoLoading == true {
                    Divider()
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Looking up biography…")
                            .font(.role(.control, system: .callout))
                            .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    }
                    .transition(.opacity)
                }
            }
            .padding(22)
            .animation(.easeOut(duration: 0.2), value: info?.bio)
            .animation(.easeOut(duration: 0.2), value: record?.infoLoading)
        }
        // Only for a library change — the artist arrived before the page did.
        .reloading(on: artistId) { await library.prepare(artist: artistId) }
        // The same pick as the album browser's, over this artist's records
        // alone. It ends with the page.
        .onChange(of: ui.selectAllToken) { _, _ in
            guard onStage else { return }
            library.artistSelection.selectAll()
        }
        .onChange(of: ui.clearSelectionToken) { _, _ in library.artistSelection.end() }
        .onChange(of: onStage) { _, now in if !now { library.artistSelection.end() } }
        .onChange(of: artistId) { _, _ in library.artistSelection.end() }
        .onDisappear { library.artistSelection.end() }
        #if os(iOS)
        .playableSelectMode(library.artistSelection, engine: library.engine, available: !albums.isEmpty)
        #endif
    }

    private func shufflePlay() {
        let engine = library.engine
        let id = artistId
        Task {
            let ids = ((try? await engine.randomTracks(count: 50, artistId: id)) ?? []).map(\.id)
            player.playNow(trackIds: ids)
            nav.showQueueWhenReady(watching: player)
        }
    }
}

/// The opening of the artist's Wikipedia article, credited as its licence asks.
private struct ArtistBio: View {
    let bio: String
    let source: String?
    let imageCredit: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("About").koanCase()
                .font(.role(.body, system: .headline))
            // The extract separates paragraphs with a single newline.
            Text(bio.replacingOccurrences(of: "\n", with: "\n\n"))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                .lineSpacing(3)
                .selectableText()
                .frame(maxWidth: 680, alignment: .leading)
            HStack(spacing: 12) {
                if let url = source.flatMap(URL.init(string:)) {
                    #if os(tvOS)
                    // A television opens no web pages; the credit stands as text.
                    Text("From Wikipedia")
                        .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                        .accessibilityHint(url.absoluteString)
                    #else
                    Link("From Wikipedia", destination: url)
                    #endif
                }
                if let imageCredit {
                    Text("Photo: \(imageCredit)")
                        .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                }
            }
            .font(.role(.fine, system: .caption))
        }
    }
}

/// Wrapping row of chips. SwiftUI still has no built-in flow layout.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 400
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x + size.width > width, x > 0 {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: width, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x + size.width > bounds.maxX, x > bounds.minX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            view.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}
