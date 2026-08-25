import KoanFFI
import SwiftUI

/// The transport, phone-sized.
///
/// Not the Mac's `TransportBar` shrunk: that one carries a seek bar, a format
/// badge, an output device picker and a radio toggle across a window that is
/// always at least 940 points wide. At 400 there is room for the sleeve, what is
/// playing and one button, and everything else belongs on the page you get by
/// tapping it.
struct MiniPlayer: View {
    @Environment(PlayerModel.self) private var player
    @Binding var showingNowPlaying: Bool

    private var entry: QueueItem? { player.nowPlaying.entry }

    var body: some View {
        HStack(spacing: 10) {
            if let source {
                AlbumArtwork(source: source, size: .thumb, cornerRadius: 5)
                    .frame(width: 32, height: 32)
            } else {
                RoundedRectangle(cornerRadius: 5)
                    .fill(.quaternary)
                    .frame(width: 32, height: 32)
                    .overlay { Image(systemName: "music.note").font(.caption) }
            }

            VStack(alignment: .leading, spacing: 1) {
                Text(entry?.title ?? "Nothing playing")
                    .font(.subheadline.weight(.medium))
                    .lineLimit(1)
                if let artist = entry?.artist, !artist.isEmpty {
                    Text(artist)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Button {
                player.togglePlayPause()
            } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                    .font(.title3)
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.plain)
            .disabled(entry == nil)

            Button { player.next() } label: {
                Image(systemName: Icon.next).font(.body)
            }
            .buttonStyle(.plain)
            .disabled(entry == nil)
        }
        .padding(.horizontal, 4)
        // The whole bar opens Now Playing; the buttons keep their own taps.
        .contentShape(Rectangle())
        .onTapGesture { if entry != nil { showingNowPlaying = true } }
        .accessibilityElement(children: .contain)
        .accessibilityHint("Opens Now Playing")
    }

    private var source: AlbumArtwork.Source? {
        guard let entry else { return nil }
        if let albumId = entry.albumId { return .album(albumId) }
        if let trackId = entry.trackId { return .track(trackId) }
        return nil
    }
}
