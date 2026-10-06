import KoanFFI
import SwiftUI

/// The transport, phone-sized.
///
/// Not the Mac's `TransportBar` shrunk: that one carries a seek bar, a format
/// badge and an output device picker across the width of a Mac window. At 400 points there is room for the sleeve, what is
/// playing and one button, and everything else belongs on the page you get by
/// tapping it.
struct MiniPlayer: View {
    @Environment(PlayerModel.self) private var player
    @Binding var showingNowPlaying: Bool
    @Binding var showingDevices: Bool

    private var entry: QueueItem? { player.currentEntry }

    var body: some View {
        HStack(spacing: 10) {
            sleeve

            VStack(alignment: .leading, spacing: 1) {
                // A play still finding its tracks, named from the tap rather
                // than leaving the paused track it replaces on show.
                Text(player.resolving ?? entry?.title ?? "Nothing playing")
                    .font(.role(.meta, system: .subheadline.weight(.medium)))
                    .lineLimit(1)
                if player.isControllingAnother {
                    // Where it is playing matters more than who by, when it
                    // is not here.
                    Label(player.controlled?.name ?? "Another device", systemImage: "laptopcomputer.and.iphone")
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(Color.accentColor)
                        .lineLimit(1)
                } else if player.resolving == nil, let artist = entry?.artist, !artist.isEmpty {
                    Text(artist)
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            HStack(spacing: 0) {
                // Reachable with nothing playing here, which is when a phone
                // is most likely to be wanted as a remote.
                if player.hasOtherDevices || player.isControllingAnother {
                    ControlButton(open: $showingDevices, labelled: false)
                        .font(.role(.body, system: .body))
                        .frame(width: Self.target, height: Self.target)
                }

                Button {
                    player.togglePlayPause()
                } label: {
                    Group {
                        if player.isWaitingForTrack {
                            ProgressView()
                        } else {
                            Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                                .font(.role(.titleSmall, system: .title3))
                                .contentTransition(.symbolEffect(.replace))
                        }
                    }
                    .frame(width: Self.target, height: Self.target)
                    .contentShape(Rectangle())
                }
                .accessibilityLabel(player.isWaitingForTrack ? "Loading" : player.isPlaying ? "Pause" : "Play")
                .disabled(entry == nil)

                Button { player.next() } label: {
                    Image(systemName: Icon.next)
                        .font(.role(.body, system: .body))
                        .frame(width: Self.target, height: Self.target)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel("Next")
                .disabled(entry == nil)
            }
            .buttonStyle(.plain)
        }
        // The bar is a capsule of fixed height, so its ends are half-circles:
        // a square sleeve needs to sit well in from one to clear the curve,
        // and the last button's glyph as far in from the other.
        .padding(.leading, 12)
        .padding(.trailing, 8)
        // The whole bar opens Now Playing; the buttons keep their own taps.
        .contentShape(Rectangle())
        .onTapGesture { if entry != nil { showingNowPlaying = true } }
        .accessibilityElement(children: .contain)
        .accessibilityHint("Opens Now Playing")
    }

    /// A thumb's worth, rather than the glyph's own few points.
    private static let target = 40.0
    private static let sleeveSide = 30.0

    @ViewBuilder private var sleeve: some View {
        if let source = player.currentArtwork {
            AlbumArtwork(source: source, size: .thumb, cornerRadius: KoanTheme.radius(7))
                .frame(width: Self.sleeveSide, height: Self.sleeveSide)
        } else {
            RoundedRectangle(cornerRadius: KoanTheme.radius(7))
                .fill(.quaternary)
                .frame(width: Self.sleeveSide, height: Self.sleeveSide)
                .overlay { Image(systemName: "music.note").font(.role(.fine, system: .caption)) }
        }
    }
}
