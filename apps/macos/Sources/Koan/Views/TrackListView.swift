import KoanFFI
import SwiftUI

/// A header and a list of tracks — the shape every detail screen takes,
/// whether it came from an album, an artist, or the favourites list.
struct TrackListView: View {
    let title: String
    var subtitle: String = ""
    let tracks: [Track]
    var artwork: AlbumArtwork.Source?

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var selection: Set<Int64> = []

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0, pinnedViews: []) {
                header
                    .padding(.horizontal, 24)
                    .padding(.top, 18)
                    .padding(.bottom, 16)

                if tracks.isEmpty {
                    EmptyState(icon: "music.note.list", title: "No tracks")
                        .frame(maxWidth: .infinity, minHeight: 220)
                } else {
                    ForEach(Array(tracks.enumerated()), id: \.element.id) { index, track in
                        TrackRow(
                            track: track,
                            position: index + 1,
                            isCurrent: player.currentTrackId == track.id,
                            isSelected: selection.contains(track.id)
                        )
                        .contentShape(.rect)
                        .onTapGesture(count: 2) {
                            player.playNow(trackIds: tracks.map(\.id), startingAt: index)
                        }
                        .onTapGesture { selection = [track.id] }
                        .contextMenu {
                            Button("Play") {
                                player.playNow(trackIds: tracks.map(\.id), startingAt: index)
                            }
                            Button("Add to Queue") { player.enqueue(trackIds: [track.id]) }
                            Divider()
                            Button(track.isFavourite ? "Remove Favourite" : "Favourite") {
                                player.toggleFavourite(trackId: track.id)
                                library.refreshFavourites()
                            }
                        }
                    }
                    .padding(.horizontal, 14)
                }
            }
            .padding(.bottom, 20)
        }
    }

    private var header: some View {
        HStack(alignment: .bottom, spacing: 18) {
            if let artwork {
                AlbumArtwork(source: artwork, cornerRadius: 8)
                    .frame(width: 132, height: 132)
                    .shadow(color: .black.opacity(0.3), radius: 10, y: 4)
            }

            VStack(alignment: .leading, spacing: 6) {
                Text(title)
                    .font(.system(size: 26, weight: .semibold))
                    .lineLimit(2)
                if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 10) {
                    Button {
                        player.playNow(trackIds: tracks.map(\.id))
                    } label: {
                        Label("Play", systemImage: "play.fill")
                    }
                    .buttonStyle(.borderedProminent)

                    Button {
                        player.enqueue(trackIds: tracks.map(\.id))
                    } label: {
                        Label("Queue", systemImage: "text.append")
                    }
                }
                .disabled(tracks.isEmpty)
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

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 12) {
            // The row number becomes a speaker for whatever is playing —
            // same width either way so the column doesn't twitch.
            Group {
                if isCurrent {
                    Image(systemName: player.isPlaying ? "speaker.wave.2.fill" : "speaker.fill")
                        .foregroundStyle(.tint)
                } else {
                    Text("\(position)")
                        .foregroundStyle(.tertiary)
                }
            }
            .font(.caption.monospacedDigit())
            .frame(width: 22, alignment: .trailing)

            VStack(alignment: .leading, spacing: 1) {
                Text(track.title)
                    .lineLimit(1)
                    .foregroundStyle(isCurrent ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
                Text(track.artistName)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            Button {
                player.toggleFavourite(trackId: track.id)
                library.refreshFavourites()
            } label: {
                Image(systemName: track.isFavourite ? "heart.fill" : "heart")
                    .foregroundStyle(track.isFavourite ? AnyShapeStyle(.red) : AnyShapeStyle(.tertiary))
            }
            .buttonStyle(.plain)
            .opacity(track.isFavourite || hovering ? 1 : 0)

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
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background {
            RoundedRectangle(cornerRadius: 6)
                .fill(isSelected ? AnyShapeStyle(.selection) : AnyShapeStyle(hovering ? AnyShapeStyle(.quaternary.opacity(0.5)) : AnyShapeStyle(.clear)))
        }
        .onHover { hovering = $0 }
    }
}
