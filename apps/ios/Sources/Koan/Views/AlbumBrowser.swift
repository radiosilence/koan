import KoanFFI
import SwiftUI

struct AlbumBrowser: View {
    @Environment(EngineMirror.self) private var mirror
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui
    @Environment(Navigator.self) private var nav
    /// Where the grid is scrolled to, as a distance rather than an album: a
    /// reshuffle reorders every album, and following the one at the top would
    /// carry the grid to wherever it landed.
    @State private var position = ScrollPosition()

    private let columns = GridItem.tiles(minimum: 150, maximum: 210, spacing: 18)

    var body: some View {
        albums
            // Once narrowed, how many: the count a shelf's heading gave. The
            // Mac's header says it already.
            .pageSubtitle(library.isNarrowed ? Format.count(Int64(library.visibleAlbums.count), "album") : "")
            // ⌘A picks everything the filter is showing, starting a selection
            // if there was none. Escape and leaving the page drop it.
            .onChange(of: ui.selectAllToken) { _, _ in library.selection.selectAll() }
            .onChange(of: ui.clearSelectionToken) { _, _ in library.selection.end() }
            .onDisappear { library.selection.end() }
    }

    private var emptyDetail: String {
        library.emptyLibraryDetail
    }

    private var empty: some View {
        EmptyState(
            icon: Icon.album,
            title: library.isNarrowed ? "Nothing matches" : "No albums yet",
            detail: library.isNarrowed
                ? "Try a different filter."
                : mirror.signInRefused ? EngineMirror.signInRefusedDetail : emptyDetail
        )
        .frame(maxWidth: .infinity, minHeight: 340)
        .task { if library.signedIn == nil { library.loadStats() } }
    }

    private var albums: some View {
        ScrollView {
            if library.visibleAlbums.isEmpty {
                empty
            } else {
                LazyVGrid(columns: columns, spacing: 22) {
                    ForEach(library.visibleAlbums, id: \.id) { album in
                        AlbumGridCell(album: album, selection: library.selection)
                    }
                }
                .padding(20)
                .modifier(SelectionDrag(selection: library.selection))
            }
        }
        // Rebuilt on each visit rather than kept mounted behind other pages
        // (see `StageView`), and put back where it was.
        .scrollPosition($position)
        .onScrollGeometryChange(for: CGFloat.self) { $0.contentOffset.y } action: { _, y in
            library.albumsOffset = y
        }
        .onAppear {
            if let y = library.albumsOffset { position.scrollTo(y: y) }
        }
        .onChange(of: nav.rewinds[.albums]) { position.scrollTo(edge: .top) }
    }
}

struct AlbumDetailView: View {
    let albumId: Int64

    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav

    /// Whatever the navigator loaded before it brought us here, so the first
    /// body evaluation already has the whole page.
    private var record: LibraryModel.AlbumRecord? {
        library.detailRecord(album: albumId)
    }

    var body: some View {
        Trace.event("album-body")
        FrameTimer.shared.evaluated()
        return page
            // For a library change — the record itself arrived before the page
            // did, and a download landing writes a cached path onto one of
            // these rows — or for a record let go while the page was kept.
            .reloading(on: Held(albumId: albumId, held: record != nil)) {
                await library.prepare(album: albumId)
            }
    }

    private struct Held: Equatable {
        let albumId: Int64
        let held: Bool
    }

    /// Never a record page with nothing on it: still reading, gone from the
    /// library, or the read failed, each said as such.
    @ViewBuilder private var page: some View {
        if let record, record.album != nil {
            TrackListView(
                title: record.album?.title ?? "",
                subtitle: subtitle,
                tracks: record.tracks,
                artwork: .album(albumId),
                artistCredit: record.album?.artistCredit,
                playable: record.album.map { Playable.album($0) }
            )
        } else if let record {
            VStack(spacing: 16) {
                EmptyState(
                    icon: Icon.unknownAlbum,
                    title: record.failed ? "Couldn't read this album" : "This album isn't in the library any more"
                )
                HStack {
                    if nav.canGoBack {
                        Button("Go Back") { nav.goBack() }
                    }
                    // A record gone from the library is read again when the
                    // library next moves; a failed read is worth retrying.
                    if record.failed {
                        Button("Try Again") { Task { await library.prepare(album: albumId) } }
                    }
                }
                .koanButtons(.standard)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            SlowReadProgress()
        }
    }

    private var subtitle: String {
        guard let record, let album = record.album else { return "" }
        var parts = [album.artistCredit.text]
        if let year = album.year { parts.append(String(year)) }
        if let codec = album.codec { parts.append(codec.uppercased()) }
        let total = record.tracks.compactMap(\.durationMs).reduce(0, +)
        if total > 0 {
            parts.append(Format.duration(total))
        }
        return parts.joined(separator: " · ")
    }
}

/// Nothing, then a spinner if the read is still going after a beat. The
/// library is local, so a record nearly always lands within a frame or two,
/// and a spinner that flashes on every page says there was something to wait
/// for when there was not.
private struct SlowReadProgress: View {
    @State private var slow = false

    var body: some View {
        Group {
            if slow { ProgressView() }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task {
            try? await Task.sleep(for: .milliseconds(300))
            slow = true
        }
    }
}

struct EmptyState: View {
    let icon: String
    let title: String
    var detail: String?

    var body: some View {
        if KoanTheme.isOn {
            KoanUnavailable(title, icon: icon, detail: detail)
        } else {
            platform
        }
    }

    private var platform: some View {
        VStack(spacing: 10) {
            KoanIcon(icon)
                .font(.role(.display, system: .system(size: 32, weight: .light)))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
            Text(title)
                .font(.role(.titleSmall, system: .title3))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            if let detail {
                Text(detail)
                    .font(.role(.control, system: .callout))
                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
            }
        }
    }
}

/// The page as a drag container, so that dragging a ticked item carries every
/// tick in the order they were made, and an unticked one carries itself.
///
/// Worked out at drag time rather than handed to the container as its
/// selection: that would be a read of the ticks in the grid's body, and every
/// tick would re-diff the grid. What it costs is the preview — a stack of ticks drags as
/// the one item under the pointer.
struct SelectionDrag: ViewModifier {
    let selection: PlayableSelection

    func body(content: Content) -> some View {
        content
    }
}
