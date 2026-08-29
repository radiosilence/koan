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

    /// The bar's outer width, so it can shed rather than squash. The sidebar
    /// and the lyrics panel both take their room out of this, so the window
    /// being wide is no promise that the bar is.
    @State private var barWidth: CGFloat = 0

    /// Wide enough to read as a slab rather than a pill at this height.
    private static let radius: CGFloat = 26
    /// One height for all three zones, so the bar is a bar and not a stack of
    /// controls that happen to be near each other.
    private static let zoneHeight: CGFloat = 46
    private static let zoneGap: CGFloat = 18
    private static let inset: CGFloat = 16

    var body: some View {
        // Three columns: a centre sized to the room there is, and two flexible
        // sides that split what is left equally, so the transport stays centred
        // as the window grows.
        //
        // The centre is *given* a width rather than allowed to claim one. It
        // held `maxWidth: 560` at the highest layout priority, which meant it
        // took 560 of any bar wide enough to offer it and left the sides to
        // share the remainder — at the smallest window that was 30pt between
        // them, and the format badge and the output device were squeezed to
        // slivers rather than being dropped.
        HStack(spacing: Self.zoneGap) {
            nowPlaying
                .frame(height: Self.zoneHeight)
                .frame(maxWidth: .infinity, alignment: .leading)

            VStack(spacing: 5) {
                controls
                SeekBar()
            }
            .frame(height: Self.zoneHeight)
            .frame(width: centreWidth)

            trailing
                .frame(height: Self.zoneHeight)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .padding(.horizontal, Self.inset)
        .padding(.vertical, 9)
        .glass(.regular, fallback: .regularMaterial, in: .rect(cornerRadius: Self.radius))
        .padding(.horizontal, Self.inset)
        .padding(.bottom, 14)
        // Measured outside the paddings, so nothing the layout below decides
        // feeds back into the width it was decided from.
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { barWidth = $0 }
    }

    /// What the three zones have between them, once both insets are paid.
    private var available: CGFloat { max(0, barWidth - Self.inset * 4) }

    /// Room enough to read a seek bar off, and no more than the controls need.
    /// A share of the bar rather than a constant: the transport is the middle
    /// of the window, and it should look like it at every width.
    private var centreWidth: CGFloat { min(560, max(210, available * 0.40)) }

    /// What each side gets. Both are the same by construction — that is what
    /// keeps the centre centred — so one number answers for both.
    private var sideWidth: CGFloat { max(0, (available - Self.zoneGap * 2 - centreWidth) / 2) }

    /// Below this the radio toggle is its icon alone. The word is the first
    /// thing worth giving up: the button is lit when it is on, so it says what
    /// it is either way.
    private var compact: Bool { sideWidth < 190 }

    /// And below this the format badge goes too. It is the last thing dropped
    /// because it is the one thing on the bar that says what the DAC is being
    /// handed — but a badge crushed to a sliver says nothing at all.
    private var showsFormat: Bool { sideWidth >= 145 }

    /// The sleeve is the leading side's own version of the same trade. It is
    /// the largest thing there and the least informative — the queue behind it
    /// is nothing but sleeves — so a narrow bar spends its room on the names
    /// instead, rather than truncating them to three letters beside a cover.
    private var showsArtwork: Bool { sideWidth >= 150 }

    // MARK: - Left: what's on

    @ViewBuilder
    private var nowPlaying: some View {
        HStack(spacing: 11) {
            if showsArtwork {
                if let sleeve = player.currentArtwork {
                    AlbumArtwork(source: sleeve, size: .thumb, cornerRadius: 8)
                        .frame(width: 44, height: 44)
                        .showsArtworkFullSize(
                            source: sleeve,
                            title: player.currentEntry?.title ?? "",
                            subtitle: player.currentEntry.map {
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
            // No minimum: a narrow bar should truncate the names, which is what
            // a line of text is for, rather than hold a width that pushes the
            // heart off the end of the zone.
            .frame(maxWidth: .infinity, alignment: .leading)
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
            if let format = player.currentFormat, showsFormat {
                Text(Format.quality(format))
                    .font(.caption.monospaced())
                    // One line or none. Mid-resize the zone is briefly narrower
                    // than the badge, and a badge that wraps to two lines is
                    // taller than the bar it sits in.
                    .lineLimit(1)
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
                if compact {
                    Label("Radio", systemImage: Icon.radio)
                        .labelStyle(.iconOnly)
                        .font(.caption)
                } else {
                    Label("Radio", systemImage: Icon.radio)
                        .labelStyle(.titleAndIcon)
                        .font(.caption)
                }
            }
            .toggleStyle(.button)
            .buttonStyle(.glass)
            .help("Radio (⌥⌘R) — when the queue runs low, keep it topped up with similar tracks")

            DeviceMenu()
        }
        // Natural size, always. What does not fit is dropped above rather than
        // compressed — a badge and a menu squeezed to a few points wide is what
        // the smallest window used to show.
        .fixedSize(horizontal: true, vertical: false)
    }
}

/// Drag to scrub. The drag value takes precedence until it's released.
///
/// Nothing here is told the position as it moves. The engine sends an anchor
/// when the playhead stops being predictable — a seek, a pause, a new track —
/// and between those the bar is *animated* to the end of the track over
/// however long is left of it. That is one animation per anchor rather than a
/// redraw per tick, and it runs where animations run rather than on this
/// thread. The elapsed figure is a system-drawn timer for the same reason.
private struct SeekBar: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        HStack(spacing: 8) {
            elapsed
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 40, alignment: .trailing)

            GeometryReader { geo in
                // Drawn rather than sized. Capsules whose `frame(width:)`
                // followed the position invalidated layout ten times a second,
                // and AppKit answered each one with a full window Auto Layout
                // pass. Same bar, same marks; only the pixels change now.
                ZStack {
                    // What has not arrived, drawn quieter than the rest.
                    //
                    // The dimming is on the tail rather than the head on
                    // purpose: a track on disk is the ordinary case and should
                    // look like the ordinary bar, so a download finishing
                    // changes nothing about what is drawn. Lighting the
                    // downloaded part instead meant the bar went *dark* the
                    // moment a transfer completed, which is exactly backwards.
                    Canvas { context, size in
                        context.fill(Self.mark(in: size, fraction: 1), with: .style(.quaternary))
                    }
                    .opacity(0.4)
                    // What can be played: the whole bar for anything already
                    // here, and as far as the bytes reach for anything still
                    // arriving. Where the track can also be seeked, the engine
                    // stops a scrub at this same extent, so the bar never
                    // offers a position playback would refuse.
                    Canvas { context, size in
                        context.fill(Self.mark(in: size, fraction: fetched ?? 1), with: .style(.quaternary))
                    }
                    // Not the tint. The tint is the colour of the record now,
                    // and a muted sleeve puts the played portion at the same
                    // value as the track behind it — this is a bar you read a
                    // position off, not a thing that needs to say whose it is.
                    //
                    // Both the played extent and the head, as layers rather
                    // than as marks redrawn at each position — see
                    // `SeekProgress`. The head is what the extent cannot show
                    // on its own: a third of a minute into nine hours is a
                    // tenth of a percent of the bar, narrower than the bar is
                    // thick, and a capsule that short is not drawn at all.
                    SeekProgress(
                        fraction: reached,
                        remaining: runway,
                        thickness: Self.thickness
                    )
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

            Text(Format.duration(player.durationMs))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 40, alignment: .leading)
        }
        .disabled(player.durationMs == 0)
    }

    private static let thickness = 4.0

    /// A capsule covering `fraction` of the bar, centred vertically. Shorter
    /// than its own thickness it would draw as a squashed dot, so it doesn't.
    private static func mark(in size: CGSize, fraction: Double) -> Path {
        let width = size.width * fraction.clamped()
        guard width >= thickness else { return Path() }
        return Capsule().path(
            in: CGRect(x: 0, y: (size.height - thickness) / 2, width: width, height: thickness)
        )
    }

    /// Where the bar starts from: the drag if there is one, otherwise the
    /// playhead as of now.
    private var reached: Double {
        if let scrubbing = player.scrubbing { return scrubbing }
        guard player.durationMs > 0 else { return 0 }
        return Double(player.playhead.at(within: player.durationMs)) / Double(player.durationMs)
    }

    /// How much of the track is left to animate through, and zero whenever the
    /// bar should simply stay where it was put — paused, stopped, dragged, or
    /// a stream that cannot yet say how long it is.
    private var runway: TimeInterval {
        guard player.scrubbing == nil, player.playhead.playing, player.durationMs > 0 else {
            return 0
        }
        let position = player.playhead.at(within: player.durationMs)
        return Double(player.durationMs - position) / 1000
    }

    /// The elapsed figure, drawn by the system from the same anchor.
    ///
    /// `Text(timerInterval:)` counts on its own without this view being
    /// evaluated again, which is the whole point: a label that ticks once a
    /// second is a second's worth of SwiftUI work in this window.
    @ViewBuilder private var elapsed: some View {
        let playhead = player.playhead
        if let scrubbing = player.scrubbing {
            Text(Format.duration(UInt64(scrubbing * Double(player.durationMs))))
        } else if playhead.playing, player.durationMs > 0 {
            // The moment the track would have started, so counting up from it
            // reads as the elapsed time.
            let started = playhead.at.addingTimeInterval(-Double(playhead.positionMs) / 1000)
            Text(
                timerInterval: started...started.addingTimeInterval(Double(player.durationMs) / 1000),
                countsDown: false
            )
        } else {
            Text(Format.duration(playhead.at(within: player.durationMs)))
        }
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
