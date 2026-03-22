import SwiftUI

struct NowPlayingView: View {
    @Environment(PlaybackManager.self) private var playback
    @Environment(\.dismiss) private var dismiss
    @State private var showQueue = false

    var body: some View {
        let np = playback.nowPlaying

        VStack(spacing: 0) {
            Capsule()
                .fill(.quaternary)
                .frame(width: 36, height: 5)
                .padding(.top, 8)

            Spacer()

            CoverArtView(url: nil)
                .padding(.horizontal, 40)

            Spacer().frame(height: 32)

            VStack(spacing: 4) {
                Text(np.track?.title ?? "Not Playing")
                    .font(.title2.bold())
                    .lineLimit(1)

                Text(np.track?.artist ?? "")
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)

                Text(np.track?.album ?? "")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 24)

            Spacer().frame(height: 24)

            SeekBar(
                positionMs: np.positionMs,
                durationMs: np.durationMs,
                onSeek: { playback.seek(to: $0) }
            )
            .padding(.horizontal, 24)

            Spacer().frame(height: 16)

            TransportControls(
                state: np.state,
                onPrevious: { playback.previous() },
                onPlayPause: { playback.playPause() },
                onNext: { playback.next() }
            )

            Spacer().frame(height: 16)

            HStack {
                // TODO: enable once track_id is available on GqlNowPlaying
                Button {} label: {
                    Image(systemName: "heart")
                        .font(.title3)
                }
                .disabled(true)
                .foregroundStyle(.secondary)

                Spacer()

                if let track = np.track {
                    Text("\(track.codec) \(track.sampleRate / 1000)kHz/\(track.bitDepth)bit")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .monospacedDigit()
                }

                Spacer()

                Button { showQueue = true } label: {
                    Image(systemName: "list.bullet")
                        .font(.title3)
                }
            }
            .padding(.horizontal, 24)

            Spacer()
        }
        .sheet(isPresented: $showQueue) {
            QueueView()
                .environment(playback)
        }
    }
}
