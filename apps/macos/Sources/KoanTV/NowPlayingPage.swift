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
            Label("Nothing playing", systemImage: "music.note")
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
            AlbumArtwork(source: source, size: .tile, cornerRadius: 16)
                .shadow(color: .black.opacity(0.35), radius: 40, y: 20)
                .transition(.opacity)
        } else {
            RoundedRectangle(cornerRadius: 16)
                .fill(.quaternary)
                .overlay { Image(systemName: "music.note").font(.system(size: 120)) }
        }
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 22) {
            if let controlled = player.controlled, player.isControllingAnother {
                Text("Playing on \(controlled.name)")
                    .font(.callout.weight(.semibold))
                    .textCase(.uppercase)
                    .foregroundStyle(.secondary)
            }
            if let entry = player.currentEntry {
                Text(entry.title)
                    .font(.system(size: 56, weight: .bold))
                    .lineLimit(2)
                Text(entry.album.isEmpty ? entry.artist : "\(entry.artist) — \(entry.album)")
                    .font(.title3)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            if let format = player.currentFormat {
                Text(Format.quality(format))
                    .font(.callout.monospaced())
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 5)
                    .background(.quaternary, in: Capsule())
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
            Button { player.previous() } label: { Image(systemName: Icon.previous) }
            Button { player.togglePlayPause() } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                    .contentTransition(.symbolEffect(.replace))
            }
            .focused($focus, equals: .playPause)
            .prefersDefaultFocus(in: page)
            Button { player.next() } label: { Image(systemName: Icon.next) }
            if let trackId = player.currentTrackId {
                TrackHeart(trackId: trackId, size: .title3)
            }
            Button { ui.toggleLyrics() } label: {
                Image(systemName: Icon.lyrics).symbolVariant(ui.showLyrics ? .fill : .none)
            }
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
                   let presets = Presets(dsp: app.dsp, device: route, none: "Off") {
                    PresetMenu(presets: presets, title: route) {
                        Label(presets.current ?? presets.none, systemImage: "slider.horizontal.3")
                    }
                }
            }
        }
        .focusSection()
        .font(.title3)
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
        SeekBar()
            .padding(.vertical, 12)
            .padding(.horizontal, 16)
            .background(
                RoundedRectangle(cornerRadius: 14)
                    .fill(.white.opacity(focused ? 0.18 : 0))
                    .stroke(.white.opacity(focused ? 0.6 : 0), lineWidth: 2)
            )
            .scaleEffect(focused ? 1.02 : 1)
            .animation(.easeOut(duration: 0.15), value: focused)
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
                Text("Up next")
                    .font(.callout.weight(.semibold))
                    .textCase(.uppercase)
                    .foregroundStyle(.secondary)
                ScrollView(.horizontal) {
                    LazyHStack(spacing: 32) {
                        ForEach(Array(upcoming), id: \.queueItemId) { item in
                            Button { player.play(itemId: item.queueItemId) } label: {
                                HStack(spacing: 18) {
                                    Group {
                                        if let source = Self.artwork(of: item) {
                                            AlbumArtwork(source: source, size: .thumb, cornerRadius: 8)
                                        } else {
                                            RoundedRectangle(cornerRadius: 8).fill(.quaternary)
                                        }
                                    }
                                    .frame(width: 96, height: 96)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(item.title).font(.callout.weight(.semibold)).lineLimit(1)
                                        Text(item.artist).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    .frame(width: 240, alignment: .leading)
                                }
                            }
                            .buttonStyle(.card)
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
