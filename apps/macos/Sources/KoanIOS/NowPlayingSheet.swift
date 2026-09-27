import KoanFFI
import SwiftUI

/// The record, full screen.
///
/// The Mac puts all of this in a bar because it has a bar's worth of width to
/// put it in. A phone does not, so the sleeve gets the screen and the controls
/// sit under it — which is also the only place a seek bar is usable with a
/// thumb. The seek bar is the Mac's own: animated from the engine's anchor
/// rather than told the position, so an open sheet costs nothing between one
/// anchor and the next.
struct NowPlayingSheet: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        VStack(spacing: 24) {
            sleeve
                .padding(.horizontal, 32)
                .padding(.top, 32)

            VStack(spacing: 4) {
                Text(player.currentEntry?.title ?? "Nothing playing")
                    .font(.title3.weight(.semibold))
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                Text(player.currentEntry?.artist ?? "")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 32)

            SeekBar().padding(.horizontal, 24)
            transport
            Spacer(minLength: 0)
        }
        .presentationDragIndicator(.visible)
    }

    @ViewBuilder private var sleeve: some View {
        if let source = player.currentArtwork {
            AlbumArtwork(source: source, size: .tile, cornerRadius: 12)
                .shadow(color: .black.opacity(0.25), radius: 24, y: 12)
        } else {
            RoundedRectangle(cornerRadius: 12)
                .fill(.quaternary)
                .aspectRatio(1, contentMode: .fit)
                .overlay { Image(systemName: "music.note").font(.largeTitle) }
        }
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
        .disabled(player.currentEntry == nil)
    }
}
