import KoanFFI
import SwiftUI

/// The record, across the room: what the television is for.
///
/// The phone's Now Playing is a sheet over the page it came from; on a
/// television it is the first page, and the one a remote returns to. The
/// sleeve and the words stand on the left at the size a room can read, the
/// controls a remote reaches sit beside them, and what comes next runs along
/// the foot. Play/Pause on the remote works wherever focus is.
struct NowPlayingPage: View {
    @Environment(PlayerModel.self) private var player
    @Environment(UIState.self) private var ui
    @Environment(AppState.self) private var app
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @State private var showingDevices = false
    @State private var showingControl = false
    @FocusState private var focus: Focus?
    @Namespace private var page

    private enum Focus: Hashable { case playPause, seek }

    var body: some View {
        Group {
            if player.currentEntry == nil {
                idle
            } else {
                VStack(alignment: .leading, spacing: 48) {
                    // One section the width of the screen: down from the tabs
                    // enters here, at play/pause, whichever control happens
                    // to sit nearest the tab that was left.
                    HStack(alignment: .center, spacing: 80) {
                        stage
                            .frame(width: 620, height: 620)
                        details
                    }
                    .focusSection()
                    UpNext()
                }
                .padding(.horizontal, 90)
                .padding(.vertical, 40)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background {
            ZStack {
                Rectangle().fill(.black)
                ArtworkBleed(source: player.currentArtwork, drifts: player.isPlaying)
                Rectangle().fill(.black.opacity(0.45))
            }
            .ignoresSafeArea()
        }
        // Whenever the remote brings focus into the page, not only when it
        // first appears.
        .defaultFocus($focus, .playPause, priority: .userInitiated)
        .focusScope(page)
        .outputSheet(isPresented: $showingDevices)
        .controlSheet(isPresented: $showingControl)
    }

    private var idle: some View {
        ContentUnavailableView {
            KoanLabel("Nothing playing", icon: Icon.track)
        } description: {
            if mirror.signInRefused {
                Text(EngineMirror.signInRefusedDetail)
            } else if library.stats?.totalTracks == 0 {
                Text(library.emptyLibraryDetail)
            } else {
                Text("Choose this Apple TV under Play on, on a phone or Mac, or pick a record from the library.")
            }
        }
        .task { if library.stats == nil { library.loadStats() } }
    }

    /// The sleeve, or the words in its place.
    @ViewBuilder private var stage: some View {
        if ui.showLyrics {
            LyricsPanel()
                .backgroundStyle(.clear)
                .transition(.opacity)
        } else if let source = player.currentArtwork {
            AlbumArtwork(source: source, size: .tile, cornerRadius: KoanTheme.radius(16))
                .koanShadow(0.35, radius: 40, y: 20)
                .transition(.opacity)
        } else {
            RoundedRectangle(cornerRadius: KoanTheme.radius(16))
                .fill(.quaternary)
                .overlay { KoanIcon(Icon.track).font(.system(size: 120)) }
        }
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 22) {
            if let controlled = player.controlled, player.isControllingAnother {
                Text("Playing on \(controlled.name)")
                    .koanText(.meta, .muted)
            }
            if let entry = player.currentEntry {
                Text(entry.title)
                    .koanText(.display, .strong)
                    .lineLimit(2)
                    .rainbowShimmer()
                Text(entry.album.isEmpty ? entry.artist : "\(entry.artist) — \(entry.album)")
                    .koanText(.titleSmall, .muted)
                    .lineLimit(1)
            }
            if let format = player.currentFormat {
                Text(Format.quality(format))
                    .koanBadge()
            }
            // The transport above the bar: down from the tabs reaches play/pause
            // first, then the bar, then what comes next, in the order they sit.
            controls
                .padding(.top, 24)
            Scrubber(focused: focus == .seek)
                .focusable()
                .focused($focus, equals: .seek)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentTransition(.opacity)
        .animation(.easeInOut(duration: 0.2), value: player.currentEntry?.queueItemId)
    }

    /// The transport, then what shapes it and where the sound goes. Each is a
    /// focusable button: a remote moves between them and clicks.
    private var controls: some View {
        HStack(spacing: 24) {
            Button { player.previous() } label: { KoanIcon(Icon.previous) }
                .koanButton(.icon)
            Button { player.togglePlayPause() } label: {
                KoanIcon(player.isPlaying ? Icon.pause : Icon.play)
                    .contentTransition(.symbolEffect(.replace))
            }
            .koanButton(.iconOutlined)
            .focused($focus, equals: .playPause)
            .prefersDefaultFocus(in: page)
            Button { player.next() } label: { KoanIcon(Icon.next) }
                .koanButton(.icon)
            if let trackId = player.currentTrackId {
                TrackHeart(trackId: trackId, size: .title3)
            }
            Button { ui.toggleLyrics() } label: {
                KoanIcon(Icon.lyrics)
                    .symbolVariant(ui.showLyrics ? .fill : .none)
                    // The theme's glyph has no filled form: on is the accent, as
                    // shuffle and repeat show it.
                    .foregroundStyle(KoanTheme.isOn && ui.showLyrics ? KoanTheme.style(.accent) : AnyShapeStyle(.foreground))
            }
            .koanButton(.icon)
            .accessibilityLabel(ui.showLyrics ? "Show artwork" : "Show lyrics")
            .accessibilityIdentifier("lyrics")
            ShuffleButton()
            RepeatButton()
            if player.hasOtherDevices || player.isControllingAnother {
                ControlButton(open: $showingControl, labelled: player.isControllingAnother)
                    .accessibilityIdentifier("play-on")
            }
            if player.canChooseOutput {
                OutputButton(open: $showingDevices, labelled: false)
                    .accessibilityIdentifier("output")
            }
            // The phone's AirPlay picker is left out: a television's audio
            // route is the system's, chosen in Control Center, and the picker,
            // a UIKit view, took the page's first focus from play/pause.
            if !player.isControllingAnother {
                if let route = app.dsp.route,
                   let presets = Presets(dsp: app.dsp, device: route) {
                    PresetMenu(presets: presets, title: route) {
                        KoanLabel(presets.summary, icon: Icon.filters)
                    }
                }
            }
        }
        .focusSection()
        .koanText(.titleSmall)
        .disabled(player.currentEntry == nil)
    }
}

/// The seek bar, which a remote moves by steps: focused, left and right go
/// back and on ten seconds at a time, as the remote's own clickpad edges do
/// in Apple's players.
private struct Scrubber: View {
    let focused: Bool
    @Environment(PlayerModel.self) private var player

    var body: some View {
        // In the theme the ring alone says it is focused.
        let lifted = focused && !KoanTheme.isOn
        SeekBar()
            .padding(.vertical, 12)
            .padding(.horizontal, 16)
            .background(
                RoundedRectangle(cornerRadius: KoanTheme.radius(14))
                    .fill(.white.opacity(lifted ? 0.18 : 0))
                    .stroke(.white.opacity(lifted ? 0.6 : 0), lineWidth: 2)
            )
            .scaleEffect(lifted ? 1.02 : 1)
            .animation(.easeOut(duration: 0.15), value: focused)
            .koanFocus()
            .onMoveCommand { direction in
                switch direction {
                case .left: player.seek(bySeconds: -10)
                case .right: player.seek(bySeconds: 10)
                default: break
                }
            }
    }
}

/// The records after this one, along the foot of Now Playing. Choosing one
/// plays it.
private struct UpNext: View {
    @Environment(PlayerModel.self) private var player

    var body: some View {
        let upcoming = player.queue
            .drop { $0.status != .playing }
            .dropFirst()
            .prefix(12)
        if !upcoming.isEmpty {
            VStack(alignment: .leading, spacing: 16) {
                KoanSectionHeader("Up next")
                ScrollView(.horizontal) {
                    LazyHStack(spacing: 32) {
                        ForEach(Array(upcoming), id: \.queueItemId) { item in
                            Button { player.play(itemId: item.queueItemId) } label: {
                                HStack(spacing: 18) {
                                    Group {
                                        if let source = Self.artwork(of: item) {
                                            AlbumArtwork(source: source, size: .thumb, cornerRadius: KoanTheme.radius(8))
                                        } else {
                                            RoundedRectangle(cornerRadius: KoanTheme.radius(8)).fill(.quaternary)
                                        }
                                    }
                                    .frame(width: 96, height: 96)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(item.title).koanText(.body, .strong).lineLimit(1)
                                        Text(item.artist).koanText(.fine, .muted).lineLimit(1)
                                    }
                                    .frame(width: 240, alignment: .leading)
                                }
                            }
                            .koanButton(.card)
                        }
                    }
                }
                .scrollClipDisabled()
            }
            // Its own section, so down from the controls lands here rather
            // than on whichever control happens to sit lowest.
            .focusSection()
        }
    }

    private static func artwork(of item: QueueItem) -> AlbumArtwork.Source? {
        if let album = item.albumId { return .album(album) }
        return item.trackId.map { .track($0) }
    }
}
