import SwiftUI

struct TransportControls: View {
    let state: PlaybackState
    let onPrevious: () -> Void
    let onPlayPause: () -> Void
    let onNext: () -> Void

    var body: some View {
        HStack(spacing: 40) {
            Button(action: onPrevious) {
                Image(systemName: "backward.fill")
                    .font(.title2)
            }

            Button(action: onPlayPause) {
                Image(systemName: state == .playing ? "pause.circle.fill" : "play.circle.fill")
                    .font(.system(size: 56))
            }

            Button(action: onNext) {
                Image(systemName: "forward.fill")
                    .font(.title2)
            }
        }
        .foregroundStyle(.primary)
    }
}
