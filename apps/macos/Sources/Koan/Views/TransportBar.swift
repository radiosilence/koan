import KoanFFI
import SwiftUI

/// The transport, as a slab of glass floating over the stage.
///
/// Inset from the window rather than welded to its bottom edge: the material
/// only reads as glass when there is something moving underneath it and an edge
/// for the light to catch. `RootView` insets the stage by the bar's measured
/// height so the queue keeps scrolling under it instead of stopping short.
///
/// It stops short of the sidebar, which is glass in its own right on macOS 26 —
/// glass floating on glass reads as neither.
struct TransportBar: View {
    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// One per direction, so the arrow that was pressed is the only one that
    /// bounces — `symbolEffect` fires on any change to the value it watches.
    @State private var backSkips = 0
    @State private var forwardSkips = 0

    /// Wide enough to read as a slab rather than a pill at this height.
    private static let radius: CGFloat = 26
    /// One height for all three zones, so the bar is a bar and not a stack of
    /// controls that happen to be near each other.
    private static let zoneHeight: CGFloat = 46

    var body: some View {
        // Three equal-weight columns so the transport stays centred as the
        // window grows, rather than the centre pinning at a fixed width and
        // everything bunching to the left.
        HStack(spacing: 18) {
            nowPlaying
                .frame(height: Self.zoneHeight)
                .frame(maxWidth: .infinity, alignment: .leading)
                .layoutPriority(1)

            VStack(spacing: 5) {
                controls
                SeekBar()
            }
            .frame(height: Self.zoneHeight)
            .frame(maxWidth: 560)
            .layoutPriority(2)

            trailing
                .frame(height: Self.zoneHeight)
                .frame(maxWidth: .infinity, alignment: .trailing)
                .layoutPriority(1)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
        .glassEffect(.regular, in: .rect(cornerRadius: Self.radius))
        .padding(.horizontal, 16)
        .padding(.bottom, 14)
    }

    // MARK: - Left: what's on

    @ViewBuilder
    private var nowPlaying: some View {
        HStack(spacing: 11) {
            if let trackId = player.currentTrackId {
                AlbumArtwork(source: .track(trackId), cornerRadius: 8)
                    .frame(width: 44, height: 44)
                    .showsArtworkFullSize(
                        source: .track(trackId),
                        title: player.nowPlaying.entry?.title ?? "",
                        subtitle: player.nowPlaying.entry.map {
                            "\($0.artist) — \($0.album)"
                        }
                    )
            } else {
                RoundedRectangle(cornerRadius: 8)
                    .fill(.quaternary)
                    .frame(width: 44, height: 44)
                    .overlay {
                        Image(systemName: "music.note")
                            .foregroundStyle(.tertiary)
                    }
            }

            VStack(alignment: .leading, spacing: 2) {
                if let entry = player.currentEntry {
                    Text(entry.title)
                        .font(.callout.weight(.medium))
                        .lineLimit(1)
                    Text(entry.artist)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else {
                    Text("Nothing playing")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(minWidth: 90, alignment: .leading)
            // Gapless means a track can change with nothing else to mark it.
            // A cross-fade catches the corner of the eye; a hard swap doesn't.
            .contentTransition(.opacity)
            .animation(.easeInOut(duration: 0.2), value: player.currentEntry?.queueItemId)

            // Next to what is playing, because that is what it acts on. ⌘D
            // does the same from anywhere, but a heart you can see also tells
            // you whether this one is already in.
            if let trackId = player.currentTrackId {
                FavouriteButton(
                    isOn: library.isFavourite(track: trackId),
                    size: .callout
                ) {
                    library.toggleFavourite(track: trackId)
                }
                .help(library.isFavourite(track: trackId)
                    ? "Remove favourite (⌘D)" : "Favourite this track (⌘D)")
            }
        }
    }

    // MARK: - Centre: transport

    private var controls: some View {
        HStack(spacing: 22) {
            // A skip is acknowledged by the arrow itself. On a remote library
            // the next track can take a moment to load, and until it does
            // nothing else on the bar has changed — so the press reads as
            // dropped, and gets repeated. Reduce Motion watches a frozen
            // value, which is how a symbol effect is opted out of: it fires
            // on a change and there is never one.
            Button {
                backSkips += 1
                player.previous()
            } label: {
                Image(systemName: "backward.fill")
                    .symbolEffect(.bounce, value: reduceMotion ? 0 : backSkips)
            }
            .keyboardShortcut(.leftArrow, modifiers: .command)
            .help("Previous track (⌘←)")

            // Bigger than the pair either side of it: it is the one you reach
            // for without looking.
            Button(action: player.togglePlayPause) {
                Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 25))
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 30)
            }
            .help(player.isPlaying ? "Pause (Space)" : "Play (Space)")

            Button {
                forwardSkips += 1
                player.next()
            } label: {
                Image(systemName: "forward.fill")
                    .symbolEffect(.bounce, value: reduceMotion ? 0 : forwardSkips)
            }
            .keyboardShortcut(.rightArrow, modifiers: .command)
            .help("Next track (⌘→)")
        }
        .buttonStyle(.plain)
        .font(.system(size: 13))
    }

    // MARK: - Right: format and output

    @ViewBuilder
    private var trailing: some View {
        HStack(spacing: 12) {
            // The whole point of the player: what the DAC is being handed.
            if let format = player.currentFormat {
                Text(Format.quality(format))
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(.quaternary, in: Capsule())
                    .help("Source format — koan matches the device rate rather than resampling")
            }

            Toggle(isOn: Binding(
                get: { player.radioEnabled },
                set: { player.setRadio($0) }
            )) {
                Label("Radio", systemImage: "dot.radiowaves.left.and.right")
                    .labelStyle(.titleAndIcon)
                    .font(.caption)
            }
            .toggleStyle(.button)
            .buttonStyle(.glass)
            .help("Radio (⌥⌘R) — when the queue runs low, keep it topped up with similar tracks")

            DeviceMenu()
        }
    }
}

/// Drag to scrub. The engine keeps reporting its own position throughout, so
/// the drag value takes precedence until it's released.
private struct SeekBar: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        HStack(spacing: 8) {
            Text(Format.duration(displayedPosition))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 40, alignment: .trailing)

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(.quaternary)
                        .frame(height: 4)
                    // Not the tint. The tint is the colour of the record now,
                    // and a muted sleeve puts the played portion at the same
                    // value as the track behind it — this is a bar you read a
                    // position off, not a thing that needs to say whose it is.
                    Capsule()
                        .fill(.primary)
                        .frame(width: geo.size.width * player.progress, height: 4)
                }
                .frame(maxHeight: .infinity, alignment: .center)
                .contentShape(.rect)
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            player.beginScrub(fraction: (value.location.x / geo.size.width).clamped())
                        }
                        .onEnded { value in
                            player.seek(fraction: (value.location.x / geo.size.width).clamped())
                        }
                )
            }
            .frame(height: 14)

            Text(Format.duration(player.clock.durationMs))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 40, alignment: .leading)
        }
        .disabled(player.clock.durationMs == 0)
    }

    private var displayedPosition: UInt64 {
        UInt64(player.progress * Double(player.clock.durationMs))
    }
}

private struct DeviceMenu: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        Menu {
            Button {
                player.setDevice(nil)
            } label: {
                Label("System Default", systemImage: player.currentDevice == nil ? "checkmark" : "")
            }
            Divider()
            ForEach(player.devices, id: \.name) { device in
                Button {
                    player.setDevice(device.name)
                } label: {
                    Label(device.name, systemImage: player.currentDevice == device.name ? "checkmark" : "")
                }
            }
        } label: {
            Image(systemName: "hifispeaker")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .frame(width: 24)
        .help("Output device — \(player.currentDevice ?? "System Default")")
    }
}

extension Double {
    func clamped() -> Double { min(1, max(0, self)) }
}
