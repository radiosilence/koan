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
        .glass(.regular, fallback: .regularMaterial, in: .rect(cornerRadius: Self.radius))
        .padding(.horizontal, 16)
        .padding(.bottom, 14)
    }

    // MARK: - Left: what's on

    @ViewBuilder
    private var nowPlaying: some View {
        HStack(spacing: 11) {
            if let sleeve = player.currentArtwork {
                AlbumArtwork(source: sleeve, size: .thumb, cornerRadius: 8)
                    .frame(width: 44, height: 44)
                    .showsArtworkFullSize(
                        source: sleeve,
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
                    // Both names go where they say they go, rather than the
                    // line as a whole meaning one of them. `LinkText` is the
                    // same one the rows and headers use — this was the last
                    // place an artist name was not a link.
                    HStack(spacing: 0) {
                        LinkText(
                            text: entry.artist,
                            target: player.currentArtistId.map { .artist($0) },
                            font: .caption
                        )
                        if !entry.album.isEmpty {
                            Text(" — ")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            LinkText(
                                text: entry.album,
                                target: player.currentAlbumId.map { .album($0) },
                                font: .caption
                            )
                        }
                    }
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
                Image(systemName: Icon.previous)
                    .symbolEffect(.bounce, value: reduceMotion ? 0 : backSkips)
            }
            .keyboardShortcut(.leftArrow, modifiers: .command)
            .help("Previous track (⌘←)")

            // Bigger than the pair either side of it: it is the one you reach
            // for without looking.
            Button(action: player.togglePlayPause) {
                Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                    .font(.system(size: 25))
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 30)
            }
            .help(player.isPlaying ? "Pause (Space)" : "Play (Space)")

            Button {
                forwardSkips += 1
                player.next()
            } label: {
                Image(systemName: Icon.next)
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
                    // A refused rate isn't a fault to raise an alarm over, so
                    // it reads at the same weight as the rest of the badge and
                    // explains itself only to someone who looks.
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(.quaternary, in: Capsule())
                    .help(Format.outputExplanation(format))
            }

            Toggle(isOn: Binding(
                get: { player.radioEnabled },
                set: { player.setRadio($0) }
            )) {
                Label("Radio", systemImage: Icon.radio)
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
                // Drawn rather than sized. Capsules whose `frame(width:)`
                // followed the position invalidated layout ten times a second,
                // and AppKit answered each one with a full window Auto Layout
                // pass. Same bar, same marks; only the pixels change now.
                ZStack {
                    Canvas { context, size in
                        context.fill(Self.mark(in: size, fraction: 1), with: .style(.quaternary))
                    }
                    // How much of a streaming track has arrived. Where the
                    // track can also be seeked, the engine stops a scrub at
                    // this same extent, so the bar never offers a position
                    // playback would refuse. Where it cannot be seeked yet,
                    // this is the whole message: the transfer is going, and
                    // this is how far.
                    //
                    // Held well back, because it is context rather than the
                    // reading. At full weight it was the brightest thing on the
                    // bar and got taken for the playhead — which on a long
                    // track is a sliver a fraction of a pixel wide, and so lost
                    // under it entirely.
                    Canvas { context, size in
                        context.fill(Self.mark(in: size, fraction: fetched ?? 0), with: .style(.secondary))
                    }
                    // On whether there is a mark at all, not on how long it is:
                    // the fetched extent moves several times a second and a
                    // quarter of a second of easing on every one of those would
                    // leave both it and the playhead beside it perpetually
                    // behind. Opacity rather than a transition, because this
                    // layer is always present — and opacity is one of the few
                    // things CoreAnimation can carry without touching layout.
                    .opacity(fetched == nil ? 0 : 0.4)
                    .animation(.easeOut(duration: 0.25), value: fetched == nil)
                    // Not the tint. The tint is the colour of the record now,
                    // and a muted sleeve puts the played portion at the same
                    // value as the track behind it — this is a bar you read a
                    // position off, not a thing that needs to say whose it is.
                    Canvas { context, size in
                        context.fill(Self.mark(in: size, fraction: player.progress), with: .style(.primary))
                    }
                    // The head itself, which the played extent cannot show on
                    // its own: a third of a minute into nine hours is a tenth
                    // of a percent of the bar, narrower than the bar is thick,
                    // and a capsule that short is not drawn at all. A mark that
                    // does not shrink with the fraction is the only thing that
                    // says where playback is on a track this long.
                    Canvas { context, size in
                        context.fill(Self.head(in: size, fraction: player.progress), with: .style(.primary))
                    }
                }
                .contentShape(.rect)
                // Always attached, even where there is nowhere to drag to. The
                // thumb does not follow the pointer then — a head that moves
                // and springs back is a worse answer than one that stays put —
                // but the attempt is still worth answering, so releasing says
                // why nothing happened.
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            guard player.canSeek else { return }
                            player.beginScrub(fraction: (value.location.x / geo.size.width).clamped())
                        }
                        .onEnded { value in
                            guard player.canSeek else { return player.explainUnseekable() }
                            player.seek(fraction: (value.location.x / geo.size.width).clamped())
                        }
                )
                .help(help)
            }
            .frame(height: 14)

            Text(Format.duration(player.clock.durationMs))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 40, alignment: .leading)
        }
        .disabled(player.clock.durationMs == 0)
    }

    private static let thickness = 4.0

    /// A capsule covering `fraction` of the bar, centred vertically. Shorter
    /// than its own thickness it would draw as a squashed dot, so it doesn't.
    /// A round head centred on `fraction`, kept inside the bar at both ends so
    /// it never hangs off the track it is marking.
    private static func head(in size: CGSize, fraction: Double) -> Path {
        let diameter = thickness * 2
        let travel = size.width - diameter
        guard travel > 0 else { return Path() }
        let x = travel * fraction.clamped()
        return Path(
            ellipseIn: CGRect(
                x: x,
                y: (size.height - diameter) / 2,
                width: diameter,
                height: diameter
            )
        )
    }

    private static func mark(in size: CGSize, fraction: Double) -> Path {
        let width = size.width * fraction.clamped()
        guard width >= thickness else { return Path() }
        return Capsule().path(
            in: CGRect(x: 0, y: (size.height - thickness) / 2, width: width, height: thickness)
        )
    }

    private var displayedPosition: UInt64 {
        UInt64(player.progress * Double(player.clock.durationMs))
    }

    /// How much of what is playing has arrived, while it is still arriving.
    /// `nil` for anything on disk, which is every track most of the time.
    private var fetched: Double? { player.fetched }

    /// Says which of the three states the bar is in, because a bar that stops
    /// short or stops responding without saying why reads as broken.
    private var help: String {
        guard let fetched else { return "" }
        let percent = Int(fetched * 100)
        return player.canSeek
            ? "Downloaded to \(percent)% — seeking stops there until the rest arrives"
            : "Downloading, \(percent)% — seeking becomes available when it finishes"
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
