import KoanFFI
import SwiftUI

/// A header and a list of tracks — the shape every detail screen takes,
/// whether it came from an album, an artist, or the favourites list.
struct TrackListView: View {
    let title: String
    var subtitle: String = ""
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
    @State private var selection: Set<Int64> = []

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
                // A real List rather than a LazyVStack of tap gestures. Stacking
                // single- and double-tap recognisers on a plain view makes
                // clicks resolve against each other and drop; List gives native
                // selection, shift/⌘ range select and keyboard navigation.
                ScrollViewReader { proxy in
                    List(selection: $selection) {
                        ForEach(Array(tracks.enumerated()), id: \.element.id) { index, track in
                            TrackRow(
                                track: track,
                                position: index + 1,
                                isCurrent: player.currentTrackId == track.id,
                                isSelected: selection.contains(track.id),
                                showsAlbum: mixedAlbums,
                                allTrackIds: tracks.map(\.id)
                            )
                            .rowBehaviour(playable: .track(track))
                        }
                    }
                    .listStyle(.inset)
                    .clearsSelection($selection)
                    // Gives up its ground only where there is a wash to show:
                    // otherwise the List paints over it and the record's colour
                    // stops in a line under the header. Without a cover — a
                    // favourites list — it keeps its ground, or it composites
                    // onto whatever the window is drawing, which is the page
                    // you came from.
                    .scrollContentBackground(artwork == nil ? .visible : .hidden)

                    // The List's own double-click hook. Wired into selection
                    // rather than the gesture system, so it doesn't steal the
                    // first click.
                    .contextMenu(forSelectionType: Int64.self) { ids in
                        menu(for: ids)
                    } primaryAction: { ids in
                        play(ids)
                    }
                    .onKeyPress(.return) {
                        playSelection()
                        return .handled
                    }
                    // Arriving from search: single out the matched track rather
                    // than dropping the user at the top of a 20-track record.
                    .task(id: tracks.count) {
                        guard let target = nav.highlightedTrackId,
                              tracks.contains(where: { $0.id == target })
                        else { return }
                        selection = [target]
                        withAnimation { proxy.scrollTo(target, anchor: .center) }
                        nav.highlightedTrackId = nil
                    }
                }
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

    private func playSelection() { play(selection) }

    /// Menu for the rows under the pointer — the List hands us the selection
    /// they belong to, so a menu on a multi-selection acts on all of it.
    @ViewBuilder
    private func menu(for ids: Set<Int64>) -> some View {
        let chosen = tracks.filter { ids.contains($0.id) }
        if chosen.count == 1, let track = chosen.first {
            PlayableMenu(playable: .track(track))
        } else if !chosen.isEmpty {
            Button("Play") {
                player.playNow(trackIds: chosen.map(\.id))
            }
            Button("Play Next") { player.playNext(trackIds: chosen.map(\.id)) }
            Button("Add to Queue") { player.enqueue(trackIds: chosen.map(\.id)) }
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

    private var header: some View {
        HStack(alignment: .bottom, spacing: 18) {
            if let artwork {
                AlbumArtwork(source: artwork, cornerRadius: 8)
                    .frame(width: 132, height: 132)
                    .shadow(color: .black.opacity(0.3), radius: 10, y: 4)
                    .showsArtworkFullSize(
                        source: artwork,
                        title: title,
                        subtitle: subtitle.isEmpty ? nil : subtitle
                    )
            }

            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 12) {
                    if let playable {
                        PlayableHeaderButton(playable: playable)
                    }
                    Text(title)
                        .font(.system(size: 26, weight: .semibold))
                        .lineLimit(2)
                }
                if let artistLink {
                    HStack(spacing: 5) {
                        LinkText(text: subtitleArtist, target: .artist(artistLink))
                        if !subtitleRest.isEmpty {
                            Text(subtitleRest)
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                    }
                } else if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 10) {
                    QueueButtons(playable: playable)
                    if let playable {
                        ShareButton(playable: playable)
                        FavouriteHeaderButton(playable: playable)
                    }
                }
                .padding(.top, 4)
            }
            Spacer(minLength: 0)
        }
    }
}

private struct TrackRow: View {
    let track: Track
    let position: Int
    let isCurrent: Bool
    let isSelected: Bool
    /// Draws the cover and names the album — see `TrackListView.mixedAlbums`.
    let showsAlbum: Bool
    /// The whole list, so playing this row keeps the rest queued behind it.
    let allTrackIds: [Int64]

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
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
                        .foregroundStyle(.tertiary)
                }
            }
            .font(.caption.monospacedDigit())
            .frame(width: 22, alignment: .trailing)

            if showsAlbum {
                // The cover is what you recognise a record by, and a list
                // gathered from the whole library is exactly that job.
                Group {
                    if let albumId = track.albumId {
                        AlbumArtwork(source: .album(albumId), size: .thumb, cornerRadius: 3)
                    } else {
                        Image(systemName: "music.note")
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                    }
                }
                .frame(width: 34, height: 34)
            }

            VStack(alignment: .leading, spacing: 1) {
                Text(track.title)
                    .lineLimit(1)
                    // Tinted to mark the playing track — but not when the row
                    // is selected, where accent-on-accent is unreadable.
                    .foregroundStyle(
                        isCurrent && !isSelected
                            ? AnyShapeStyle(.tint)
                            : AnyShapeStyle(.primary)
                    )
                HStack(spacing: 5) {
                    LinkText(
                        text: track.artistName,
                        target: track.artistId.map { .artist($0) },
                        font: .caption
                    )
                    if showsAlbum {
                        Text("·")
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                        LinkText(
                            text: track.albumTitle,
                            target: track.albumId.map { .album($0) },
                            font: .caption
                        )
                    }
                }
            }

            Spacer(minLength: 8)

            TrackAvailability(track: track)

            FavouriteButton(isOn: library.isFavourite(track: track.id), showing: hovering) {
                library.toggleFavourite(track: track.id)
            }

            if let quality = Format.quality(track) {
                Text(quality)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.tertiary)
                    .frame(width: 92, alignment: .trailing)
            }

            Text(Format.duration(track.durationMs))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 48, alignment: .trailing)
        }
        .frame(height: showsAlbum ? 44 : 34)
        // The row is only clickable where a view sits; the Spacer would
        // otherwise be a dead zone.
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
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

    @Environment(PlayerModel.self) private var player

    var body: some View {
        Group {
            if let queued = player.queuedByTrack[track.id], isLive(queued.status) {
                queueState(queued)
            } else {
                SourceBadges(track: track)
            }
        }
        .font(.caption)
        .frame(width: 30, height: 16, alignment: .trailing)
    }

    /// Only these say something the library row doesn't already know.
    private func isLive(_ status: EntryStatus) -> Bool {
        status == .downloading || status == .priorityPending || status == .failed
    }

    @ViewBuilder
    private func queueState(_ item: QueueItem) -> some View {
        switch item.status {
        case .downloading:
            if let progress = item.downloadProgress {
                ProgressView(value: progress)
                    .progressViewStyle(.circular)
                    .controlSize(.mini)
                    .frame(width: 14, height: 14)
                    .help("Downloading — \(Int(progress * 100))%")
            } else {
                ProgressView()
                    .progressViewStyle(.circular)
                    .controlSize(.mini)
            }
        case .priorityPending:
            Image(systemName: "arrow.down.circle")
                .foregroundStyle(.tint)
                .help("Queued for download")
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .help(item.failureReason ?? "Couldn't be fetched")
        default:
            EmptyView()
        }
    }
}
