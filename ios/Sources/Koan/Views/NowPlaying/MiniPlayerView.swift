import SwiftUI

struct MiniPlayerView: View {
    @Environment(PlaybackManager.self) private var playback
    let onTap: () -> Void

    var body: some View {
        let np = playback.nowPlaying

        VStack(spacing: 0) {
            GeometryReader { geo in
                Rectangle()
                    .fill(Color.accentColor)
                    .frame(width: geo.size.width * np.progress)
            }
            .frame(height: 2)

            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 4)
                    .fill(.quaternary)
                    .frame(width: 40, height: 40)
                    .overlay {
                        Image(systemName: "music.note")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                VStack(alignment: .leading, spacing: 2) {
                    Text(np.track?.title ?? "Not Playing")
                        .font(.subheadline.bold())
                        .lineLimit(1)

                    Text(np.track?.artist ?? "")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer()

                Button {
                    playback.playPause()
                } label: {
                    Image(
                        systemName: np.state == .playing ? "pause.fill" : "play.fill"
                    )
                    .font(.title3)
                    .frame(width: 44, height: 44)
                }
                .disabled(np.track == nil)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
        .background(.ultraThinMaterial)
        .contentShape(Rectangle())
        .onTapGesture(perform: onTap)
    }
}
