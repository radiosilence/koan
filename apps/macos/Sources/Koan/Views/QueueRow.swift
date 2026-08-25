import KoanFFI
import SwiftUI

/// What a row in the queue — or in a playlist — has to show.
///
/// The two lists are the same row: a mark saying what this track is doing, its
/// place, its name, a heart, its codec and its length. They differ only in what
/// they are made of, so the row is made of this instead and each builds one.
/// Writing a second row that looked like the first is how the playlist ended up
/// without a codec column and with its playing mark in the wrong place.
struct QueueRowContent {
    /// The library row behind it, where there is one. A queue can hold a file
    /// that was never indexed; nothing about it can be favourited.
    var trackId: Int64?
    var title: String
    var artist: String
    var album: String
    /// Its place: the track number in the queue, the position in a playlist.
    /// Int64 because the two sources disagree on width and this only ever gets
    /// printed.
    var number: Int64?
    var codec: String?
    var durationMs: Int64?
    /// The record to draw, asked for by album wherever one is known — art is
    /// stored per record, so asking by track draws the same image at the cost
    /// of a round trip and a cache entry each.
    var sleeve: AlbumArtwork.Source?
    /// What the queue is doing about this track, or `nil` when the queue has
    /// never heard of it — which is most of a playlist, most of the time.
    var status: EntryStatus?
    var downloadProgress: Double?
    var failureReason: String?

    init(item: QueueItem) {
        trackId = item.trackId
        title = item.title
        artist = item.artist
        album = item.album
        number = item.trackNumber
        codec = item.codec
        durationMs = item.durationMs.map(Int64.init)
        sleeve = item.sleeve
        status = item.status
        downloadProgress = item.downloadProgress
        failureReason = item.failureReason
    }

    /// A playlist row, wearing whatever the queue currently thinks of it.
    ///
    /// `queued` is found by *entry*, not by track: two copies of one song in a
    /// playlist are two rows and two queue items, and only the entry tells them
    /// apart. Which is also why the queue may be shuffled or cut about and this
    /// still answers correctly — the queue is a view onto the playlist, not a
    /// copy of it.
    ///
    /// Only the *live* states are mirrored — playing, fetching, failed. A
    /// playlist is not a queue, so "queued" and "played" say nothing about the
    /// track in front of you and would dot or dim half the list.
    init(entry: PlaylistEntry, position: Int, queued: QueueItem?, isCurrent: Bool) {
        let track = entry.track
        trackId = track.id
        title = track.title
        artist = track.artistName
        album = track.albumTitle
        number = Int64(position)
        codec = track.codec
        durationMs = track.durationMs
        sleeve = track.albumId.map { .album($0) } ?? .track(track.id)
        downloadProgress = queued?.downloadProgress
        failureReason = queued?.failureReason
        status =
            if isCurrent { .playing }
            else if let live = queued?.status, live == .downloading || live == .priorityPending
                || live == .failed { live }
            else { nil }
    }
}

struct QueueRow: View {
    let item: QueueRowContent
    let isCurrent: Bool
    let isSelected: Bool
    let showArtist: Bool
    /// Its own sleeve, for when there is no album heading above carrying one.
    var artwork = false

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            // The status and the number are one thing — where this track is and
            // what it is doing — so they sit together. Apart, a mark narrower
            // than its column and a number aligned to the right of its own left
            // most of twenty points of air between them.
            HStack(spacing: 6) {
                statusIcon
                    .font(.caption)
                    .frame(width: 16, alignment: .trailing)

                if artwork, let sleeve = item.sleeve {
                    AlbumArtwork(source: sleeve, size: .thumb, cornerRadius: 3)
                        .frame(width: 34, height: 34)
                        // The one thing a foreground style cannot dim. A leaf
                        // image has nothing under it to flatten, so its own
                        // layer costs what a row's did not.
                        .opacity(played ? 0.5 : 1)
                } else {
                    // Always occupies its column, number or not: a missing track
                    // number would otherwise shift the title left and break the
                    // alignment down the list.
                    Text(item.number.map(String.init) ?? "")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(played ? .quaternary : .tertiary)
                        .frame(width: artwork ? 34 : 20, alignment: .trailing)
                }
            }

            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .lineLimit(1)
                    .foregroundStyle(titleStyle)
                // Only worth a second line when it differs from the album
                // artist — compilations and features, not every track.
                if showArtist && !item.artist.isEmpty {
                    Text(artwork && !item.album.isEmpty
                        ? "\(item.artist) — \(item.album)"
                        : item.artist)
                        .font(.caption)
                        .foregroundStyle(played ? .tertiary : .secondary)
                        .lineLimit(1)
                }
            }

            Spacer(minLength: 8)

            if let trackId = item.trackId {
                FavouriteButton(
                    isOn: library.isFavourite(track: trackId),
                    showing: hovering,
                    size: .caption
                ) {
                    library.toggleFavourite(track: trackId)
                }
                .frame(width: 16)
            } else {
                // Keeps the column even for an item with no library row, so
                // the durations stay in line down the queue.
                Color.clear.frame(width: 16, height: 1)
            }

            if let codec = item.codec {
                Text(codec.uppercased())
                    .font(.caption2.monospaced())
                    .foregroundStyle(played ? .quaternary : .tertiary)
            }

            if let ms = item.durationMs {
                Text(Format.duration(ms))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(played ? .tertiary : .secondary)
                    .frame(width: 44, alignment: .trailing)
            }
        }
        // Fixed height so a row doesn't grow when a download indicator appears
        // and shrink when it finishes, reflowing the list each time.
        // The same 34pt sleeve in the same 44pt row as an album's tracklist —
        // a cover crammed into a row sized for a number reads as cramped
        // however much padding is put around it.
        .frame(height: artwork ? 44 : 34)
        // Ungrouped, the row carries a sleeve and two lines of text, and the
        // frame around them left the cover all but touching the separators.
        // The same six points the album heading gives its own cover — the two
        // kinds of row are in the same list and should breathe alike.
        .padding(.vertical, artwork ? 6 : 0)
        // Without this the row is only clickable where a view actually sits —
        // the Spacer between the title and the duration is a dead zone, and
        // clicks landing there select nothing.
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    /// A played row steps back rather than disappears — and it does it in
    /// colour, not in alpha.
    ///
    /// This was `.opacity(0.45)` on the whole row. Any alpha below 1 makes
    /// SwiftUI render the row into an offscreen layer and composite that,
    /// because its children overlap and would otherwise show through each
    /// other — so a queue with a long tail behind the cursor was a long list of
    /// offscreen passes, every frame it drew. A dimmer foreground style costs
    /// a colour lookup.
    ///
    /// Never true for a playlist row, whose status is nil: played is something
    /// a queue knows about its own cursor.
    private var played: Bool { item.status == .played }

    /// Tinted to mark the playing track — but not when the row is selected,
    /// where accent-on-accent is unreadable. A played row is never the current
    /// one, so the two never contend.
    private var titleStyle: AnyShapeStyle {
        if isCurrent && !isSelected { return AnyShapeStyle(.tint) }
        return AnyShapeStyle(played ? HierarchicalShapeStyle.secondary : .primary)
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch item.status {
        case nil:
            // The queue has never heard of this track. Its column stays, so
            // the titles below it still line up.
            Color.clear
        case .playing:
            PlayingIndicator(isPlaying: player.isPlaying)
        case .downloading:
            // The ring is the whole indicator — a static arrow beside a
            // separate bar said the same thing twice, in two places, and the
            // column the eye already reads for state was the mute one.
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
                    .help("Downloading")
            }
        case .priorityPending:
            Image(systemName: "arrow.down.circle")
                .foregroundStyle(.tint)
                .help("Queued for download")
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .help(item.failureReason ?? "Couldn't be fetched")
        case .played:
            Image(systemName: "checkmark").foregroundStyle(.tertiary)
        case .queued:
            Image(systemName: "circle.dotted").foregroundStyle(.quaternary)
        }
    }
}
