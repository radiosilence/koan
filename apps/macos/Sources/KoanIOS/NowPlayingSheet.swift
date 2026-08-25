import KoanFFI
import SwiftUI

/// The record, full screen.
///
/// The Mac puts all of this in a bar because it has a bar's worth of width to
/// put it in. A phone does not, so the sleeve gets the screen and the controls
/// sit under it — which is also the only place a seek bar is usable with a
/// thumb.
struct NowPlayingSheet: View {
    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @Environment(\.dismiss) private var dismiss

    /// Where the thumb is while it is down. The clock keeps ticking underneath,
    /// and a bar that jumps back to it mid-drag is unusable.
    @State private var scrubbing: Double?

    private var entry: QueueItem? { player.nowPlaying.entry }

    var body: some View {
        VStack(spacing: 24) {
            Capsule()
                .fill(.quaternary)
                .frame(width: 36, height: 5)
                .padding(.top, 8)

            sleeve
                .padding(.horizontal, 32)

            VStack(spacing: 4) {
                Text(entry?.title ?? "Nothing playing")
                    .font(.title3.weight(.semibold))
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                Text(entry?.artist ?? "")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 32)

            seekBar.padding(.horizontal, 32)
            transport
            Spacer(minLength: 0)
        }
        .presentationDragIndicator(.hidden)
    }

    @ViewBuilder private var sleeve: some View {
        if let source {
            AlbumArtwork(source: source, size: .tile, cornerRadius: 12)
                .shadow(color: .black.opacity(0.25), radius: 24, y: 12)
        } else {
            RoundedRectangle(cornerRadius: 12)
                .fill(.quaternary)
                .aspectRatio(1, contentMode: .fit)
                .overlay { Image(systemName: "music.note").font(.largeTitle) }
        }
    }

    private var seekBar: some View {
        VStack(spacing: 4) {
            Slider(
                value: Binding(
                    get: { scrubbing ?? player.progress },
                    set: { scrubbing = $0 }
                ),
                in: 0...1,
                onEditingChanged: { editing in
                    guard !editing, let target = scrubbing else { return }
                    player.seek(fraction: target)
                    scrubbing = nil
                }
            )
            .disabled(player.clock.durationMs == 0)

            HStack {
                Text(Format.duration(displayedMs))
                Spacer()
                Text(Format.duration(player.clock.durationMs))
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
    }

    private var displayedMs: UInt64 {
        guard let scrubbing else { return player.clock.positionMs }
        return UInt64(scrubbing * Double(player.clock.durationMs))
    }

    private var transport: some View {
        HStack(spacing: 44) {
            Button { player.previous() } label: {
                Image(systemName: Icon.previous).font(.title)
            }
            Button { player.togglePlayPause() } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                    .font(.system(size: 46))
                    .contentTransition(.symbolEffect(.replace))
            }
            Button { player.next() } label: {
                Image(systemName: Icon.next).font(.title)
            }
        }
        .buttonStyle(.plain)
        .disabled(entry == nil)
    }

    private var source: AlbumArtwork.Source? {
        guard let entry else { return nil }
        if let albumId = entry.albumId { return .album(albumId) }
        if let trackId = entry.trackId { return .track(trackId) }
        return nil
    }
}
