import KoanFFI
import SwiftUI

struct TransportBar: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        // Three equal-weight columns so the transport stays centred as the
        // window grows, rather than the centre pinning at a fixed width and
        // everything bunching to the left.
        HStack(spacing: 16) {
            nowPlaying
                .frame(maxWidth: .infinity, alignment: .leading)
                .layoutPriority(1)

            VStack(spacing: 5) {
                controls
                SeekBar()
            }
            .frame(maxWidth: 560)
            .layoutPriority(2)

            trailing
                .frame(maxWidth: .infinity, alignment: .trailing)
                .layoutPriority(1)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 11)
        .background(.bar)
    }

    // MARK: - Left: what's on

    @ViewBuilder
    private var nowPlaying: some View {
        HStack(spacing: 11) {
            if let trackId = player.currentTrackId {
                AlbumArtwork(source: .track(trackId), cornerRadius: 5)
                    .frame(width: 46, height: 46)
            } else {
                RoundedRectangle(cornerRadius: 5)
                    .fill(.quaternary)
                    .frame(width: 46, height: 46)
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
        }
    }

    // MARK: - Centre: transport

    private var controls: some View {
        HStack(spacing: 20) {
            Button(action: player.previous) {
                Image(systemName: "backward.fill")
            }
            .keyboardShortcut(.leftArrow, modifiers: .command)
            .help("Previous track (⌘←)")

            Button(action: player.togglePlayPause) {
                Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 19))
                    .frame(width: 26)
            }
            .help(player.isPlaying ? "Pause (Space)" : "Play (Space)")

            Button(action: player.next) {
                Image(systemName: "forward.fill")
            }
            .keyboardShortcut(.rightArrow, modifiers: .command)
            .help("Next track (⌘→)")
        }
        .buttonStyle(.plain)
        .font(.system(size: 14))
        .disabled(player.currentEntry == nil)
    }

    // MARK: - Right: format and output

    @ViewBuilder
    private var trailing: some View {
        HStack(spacing: 14) {
            // The whole point of the player: what the DAC is being handed.
            if let format = player.currentFormat {
                Text(Format.quality(format))
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
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
                    Capsule()
                        .fill(.tint)
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
