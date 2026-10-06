import KoanFFI
import SwiftUI

/// A header and a list of tracks — the shape every detail screen takes,
/// whether it came from an album, an artist, or the favourites list.
struct TrackListView: View {
    let title: String
    var subtitle = ""
    let tracks: [Track]
    var artwork: AlbumArtwork.Source?
    /// Makes the header's subtitle navigate to the artist.
    var artistLink: Int64?
    /// What the header's play button acts on.
    var playable: Playable?
    /// Set when the tracks come from all over rather than from one record.
    /// Those rows carry their own cover and name the album they came from —
    /// a single record's tracklist has both in the header above it.
    var mixedAlbums = false

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(\.horizontalSizeClass) private var width
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

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if tracks.isEmpty {
                EmptyState(icon: "music.note.list", title: emptyTitle)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                #if os(macOS)
                table
                #else
                list
                #endif
            }
        }
    }

    #if os(macOS)
    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private var table: some View {
        let numbered = tracks.enumerated().map { TrackLine(id: $1.id, kind: .track($1), lead: "\($0 + 1)", position: $0) }
        let queued = mirror.queuedByTrack
        let current = player.currentTrackId
        let playing = player.isPlaying
        let live = onStage && !reduceMotion && graphics.animatesIndicators
        // What the rows draw that can change under them, as one value. Download
        // progress is not: `TransferMeter` hands it to the rings directly.
        let key = [
            AnyHashable(current), AnyHashable(playing), AnyHashable(live), AnyHashable(tint),
            AnyHashable(library.favouriteTrackIds),
            AnyHashable(queued.map { "\($0.key):\($0.value.status)" }.sorted()),
        ]
        return SafeAreaReader { insets in
            KoanTable(
                items: numbered,
                id: \.id,
                context: TrackTableRow.Context(
                    showsAlbum: mixedAlbums,
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
                    play: { line in player.playNow(trackIds: tracks.map(\.id), startingAt: line.position) },
                    openArtist: { nav.open(artist: $0) },
                    openAlbum: { nav.open(album: $0) },
                    toggleFavourite: { library.toggleFavourite(track: $0) }
                ),
                contextKey: AnyHashable(key),
                selection: $selection,
                make: TrackTableRow.init,
                rowHeight: mixedAlbums ? TrackTableRow.artHeight : TrackTableRow.height,
                menu: { ids, environment in
                    let chosen = tracks.filter { ids.contains($0.id) }
                    if chosen.count == 1, let track = chosen.first {
                        return hostedMenu(PlayableMenu(playable: .track(track)), environment: environment)
                    }
                    return chosen.isEmpty ? nil : hostedMenu(QueueActions(trackIds: chosen.map(\.id)), environment: environment)
                },
                primaryAction: play,
                drag: { ids in tracks.filter { ids.contains($0.id) }.map { PlayableTransfer(.track($0)) } },
                selectAllToken: ui.selectAllToken,
                reveal: nav.highlightedTrackId,
                revealed: { nav.highlightedTrackId = nil },
                insets: insets
            )
        }
    }
    #endif

    private var list: some View {
                // A real List rather than a LazyVStack of tap gestures. Stacking
                // single- and double-tap recognisers on a plain view makes
                // clicks resolve against each other and drop; List gives native
                // selection, shift/⌘ range select and keyboard navigation.
                ScrollViewReader { proxy in
                    // Built once per pass rather than once per row. Every row
                    // carries the list it belongs to so playing it keeps the
                    // rest behind it, and mapping inside the `ForEach` body
                    // would allocate a fresh copy of the whole thing for each one.
                    let allTrackIds = tracks.map(\.id)
                    List(selection: $selection) {
                        ForEach(Array(tracks.enumerated()), id: \.element.id) { index, track in
                            TrackRow(
                                track: track,
                                position: index + 1,
                                showsAlbum: mixedAlbums,
                                allTrackIds: allTrackIds
                            )
                            .rowBehaviour(playable: .track(track))
                            .primaryTap { play([track.id]) } menu: { menu(for: [track.id]) }
                        }
                    }
                    .insetList()
                    .washedGround()

                    // The List's own double-click hook. Wired into selection
                    // rather than the gesture system, so it doesn't steal the
                    // first click.
                    .selectionMenu(for: Int64.self) { ids in
                        menu(for: ids)
                    } primaryAction: { ids in
                        play(ids)
                    }
                    .onKeyPress(.return) {
                        play(selection)
                        return .handled
                    }
                    // Arriving from search: single out the matched track rather
                    // than dropping the user at the top of a 20-track record.
                    .task(id: HighlightKey(target: nav.highlightedTrackId, count: tracks.count)) {
                        guard let target = nav.highlightedTrackId,
                              tracks.contains(where: { $0.id == target })
                        else { return }
                        selection = [target]
                        withAnimation { proxy.scrollTo(target, anchor: .center) }
                        nav.highlightedTrackId = nil
                    }
                }
    }

    /// Only a gathered list is narrowed by the filter; a record's tracklist
    /// with nothing in it is a record with nothing in it.
    private var emptyTitle: String {
        mixedAlbums && !library.filter.isEmpty ? "No matches" : "No tracks"
    }

    /// Plays the list from the first of `ids`, keeping the rest behind it.
    private func play(_ ids: Set<Int64>) {
        guard let index = tracks.firstIndex(where: { ids.contains($0.id) }) else { return }
        player.playNow(trackIds: tracks.map(\.id), startingAt: index)
    }

    /// Menu for the rows under the pointer — the List hands us the selection
    /// they belong to, so a menu on a multi-selection acts on all of it.
    @ViewBuilder
    private func menu(for ids: Set<Int64>) -> some View {
        let chosen = tracks.filter { ids.contains($0.id) }
        if chosen.count == 1, let track = chosen.first {
            PlayableMenu(playable: .track(track))
        } else if !chosen.isEmpty {
            QueueActions(trackIds: chosen.map(\.id))
        }
    }

    /// The subtitle is "Artist · 2007 · FLAC · 59:10"; only the first part is
    /// the artist, and only that part should link.
    private var subtitleArtist: String {
        subtitle.components(separatedBy: " · ").first ?? subtitle
    }

    private var subtitleRest: String {
        let parts = subtitle.components(separatedBy: " · ").dropFirst()
        return parts.isEmpty ? "" : "· " + parts.joined(separator: " · ")
    }

    /// Side by side where there is room, stacked where there is not.
    ///
    /// The sleeve is 132pt and the title is set at 26pt; on a phone that leaves
    /// the text column about 230pt, which is not enough for a title and four
    /// labelled buttons.
    @ViewBuilder private var header: some View {
        if width == .compact {
            VStack(alignment: .leading, spacing: 14) {
                sleeve
                titleBlock
            }
            // Without this the stack is only as wide as its widest child and
            // the parent centres the lot, which is not koan's alignment
            // anywhere else in either app.
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            HStack(alignment: .bottom, spacing: 18) {
                sleeve
                titleBlock
                Spacer(minLength: 0)
            }
        }
    }

    @ViewBuilder private var sleeve: some View {
        if let artwork {
            AlbumArtwork(source: artwork, cornerRadius: KoanTheme.radius(8))
                .frame(width: Columns.sleeve, height: Columns.sleeve)
                .shadow(color: .black.opacity(KoanTheme.isOn ? 0 : 0.3), radius: 10, y: 4)
                .showsArtworkFullSize(
                    source: artwork,
                    title: title,
                    subtitle: subtitle.isEmpty ? nil : subtitle
                )
        }
    }

    private var titleBlock: some View {
        VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: Columns.headerGap) {
                    #if !os(tvOS)
                    if let playable {
                        PlayableHeaderButton(playable: playable)
                    }
                    #endif
                    Text(Format.title(title))
                        .font(.role(.title, system: .system(size: Columns.title, weight: .semibold)))
                        .foregroundStyle(KoanTheme.style(.strong, system: .primary))
                        .lineLimit(2)
                        // Beside the play button the row offers one line's
                        // height; asked for two, it has to be let grow.
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let artistLink {
                    HStack(spacing: 5) {
                        LinkText(text: subtitleArtist, target: .artist(artistLink))
                        if !subtitleRest.isEmpty {
                            Text(subtitleRest)
                                .font(.role(.control, system: .callout))
                                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        }
                    }
                } else if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.role(.control, system: .callout))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                }

                HeaderActions(playable: playable)
                    .padding(.top, 4)
            }
    }

    /// The same record can be opened again highlighting a different track.
    private struct HighlightKey: Equatable {
        let target: Int64?
        let count: Int
    }
}

struct TrackRow: View {
    let track: Track
    let position: Int
    /// Draws the cover and names the album — see `TrackListView.mixedAlbums`.
    let showsAlbum: Bool
    /// The whole list, so playing this row keeps the rest queued behind it.
    let allTrackIds: [Int64]

    @Environment(PlayerModel.self) private var player
    /// Whether the List has this row selected — see `QueueRow.prominence`.
    @Environment(\.backgroundProminence) private var prominence
    @Environment(\.horizontalSizeClass) private var width
    @State private var hovering = false

    var body: some View {
        // Read here, in the row, rather than handed down by the list: what is
        // playing moves on every pause and every queue edit, and a list that
        // read it would re-diff every row for each. Only the rows on screen exist,
        // so this is thirty small bodies rather than one large one.
        let isCurrent = player.currentTrackId == track.id
        let isSelected = prominence == .increased

        HStack(spacing: 12) {
            // The row number becomes bars for whatever is playing —
            // same width either way so the column doesn't twitch.
            Group {
                if hovering {
                    RowPlayButton(
                        playable: .track(track),
                        visible: true,
                        inContext: (trackIds: allTrackIds, startAt: position - 1)
                    )
                } else if isCurrent {
                    PlayingIndicator(isPlaying: player.isPlaying)
                } else {
                    Text("\(position)")
                        .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                }
            }
            .font(.role(.fine, system: .caption.monospacedDigit()))
            .frame(width: 22, alignment: .trailing)

            if showsAlbum {
                // The cover is what you recognise a record by, and a list
                // gathered from the whole library is exactly that job.
                TrackSleeve(albumId: track.albumId)
                    .frame(width: RowMetrics.sleeve, height: RowMetrics.sleeve)
            }

            VStack(alignment: .leading, spacing: 1) {
                Text(Format.title(track.title))
                    .lineLimit(Format.titleLines)
                    // Tinted to mark the playing track — but not when the row
                    // is selected, where accent-on-accent is unreadable.
                    .foregroundStyle(
                        isCurrent && !isSelected
                            ? AnyShapeStyle(.tint)
                            : KoanTheme.style(.ink, system: .primary)
                    )
                HStack(spacing: 5) {
                    LinkText(
                        text: track.artistName,
                        target: track.artistId.map { .artist($0) },
                        font: .role(.fine, system: .caption)
                    )
                    if showsAlbum {
                        Text("·")
                            .font(.role(.fine, system: .caption))
                            .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                        LinkText(
                            text: track.albumTitle,
                            target: track.albumId.map { .album($0) },
                            font: .role(.fine, system: .caption)
                        )
                    }
                }
            }

            Spacer(minLength: 8)

            TrackAvailability(track: track)

            // There is no hover on a phone, so a heart that appears on it is a
            // heart that never appears.
            TrackHeart(trackId: track.id, showing: hovering || width == .compact)

            // 92pt of codec and sample rate is worth having on a window and
            // not worth a truncated title on a phone. The format is on the
            // record's header and in the transport either way.
            if width != .compact, let quality = Format.quality(track) {
                Text(quality)
                    .font(.role(.fine, system: .caption2.monospaced()))
                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                    .frame(width: Columns.quality, alignment: .trailing)
            }

            Text(Format.duration(track.durationMs))
                .font(.role(.fine, system: .caption.monospacedDigit()))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                .frame(width: Columns.duration, alignment: .trailing)
        }
        #if os(iOS) || os(tvOS)
        .frame(minHeight: showsAlbum ? RowMetrics.art : RowMetrics.text)
        #else
        .frame(height: showsAlbum ? RowMetrics.art : RowMetrics.text)
        #endif
        // The row is only clickable where a view sits; the Spacer would
        // otherwise be a dead zone.
        .contentShape(Rectangle())
        .pointerHover { hovering = $0 }
    }
}


/// A track's sleeve at row size, or a note where the library knows no record.
struct TrackSleeve: View {
    let albumId: Int64?

    var body: some View {
        if let albumId {
            AlbumArtwork(source: .album(albumId), size: .thumb, cornerRadius: KoanTheme.radius(3))
        } else {
            Image(systemName: "music.note")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
        }
    }
}

/// Whether this track can play right now, and what the queue is doing about it
/// if not.
///
/// Two different facts share one slot. The library knows whether a file exists
/// on disk; only the queue knows a download is in flight. A row shows the live
/// queue state when there is one and falls back to the library's answer.
private struct TrackAvailability: View {
    let track: Track

    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        Group {
            // Only the states the badge cannot say itself. Downloading is one
            // it can — the ring belongs in the same slot as the cloud it is
            // filling, not in a column of its own.
            if let queued = mirror.queuedByTrack[track.id], isBlocking(queued.status) {
                queueState(queued)
            } else {
                SourceBadges(track: track, queued: mirror.queuedByTrack[track.id])
            }
        }
        .font(.role(.fine, system: .caption))
        .frame(width: 30, height: 16, alignment: .trailing)
    }

    /// Only these say something neither the library row nor the badge can.
    private func isBlocking(_ status: EntryStatus) -> Bool {
        status == .priorityPending || status == .failed
    }

    @ViewBuilder
    private func queueState(_ item: QueueItem) -> some View {
        switch item.status {
        case .priorityPending:
            Image(systemName: "arrow.down.circle")
                .foregroundStyle(.tint)
                .help("Queued for download")
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(KoanTheme.style(.bad, system: .orange))
                .help(item.failureReason ?? "Couldn't be fetched")
        default:
            EmptyView()
        }
    }
}

/// The widths of a track row's fixed columns, and the header's sizes. A
/// television sets type at twice a Mac's size, is read from across a room, and
/// grows a focused button into its neighbours.
private enum Columns {
    #if os(tvOS)
    static let quality = 190.0
    static let duration = 96.0
    static let headerGap = 32.0
    static let sleeve = 260.0
    static let title = 48.0
    #else
    static let quality = 92.0
    static let duration = 48.0
    static let headerGap = 12.0
    static let sleeve = 132.0
    static let title = 26.0
    #endif
}
