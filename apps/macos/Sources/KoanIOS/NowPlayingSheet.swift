import AVKit
import KoanFFI
import SwiftUI

/// The record, full screen: the phone's transport.
///
/// The Mac spreads this along a bar because it has a bar's worth of width. A
/// phone gives the sleeve the screen and stacks the rest under it, which is
/// also the only place a seek bar is usable with a thumb. The seek bar is the
/// Mac's own — animated from the engine's anchor rather than told the position,
/// so an open sheet costs nothing between one anchor and the next.
struct NowPlayingSheet: View {
    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(UIState.self) private var ui
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 20) {
            stage
                .padding(.horizontal, 28)
                .padding(.top, 36)
                .frame(maxHeight: .infinity)

            titles.padding(.horizontal, 28)
            SeekBar().padding(.horizontal, 20)
            transport
            extras
                .padding(.horizontal, 28)
                .padding(.bottom, 12)
        }
        .presentationDragIndicator(.visible)
        // The playing record's own wash, whatever page the sheet was opened
        // over — this is the one screen that is only about that record.
        .presentationBackground {
            ZStack {
                Rectangle().fill(.background)
                ArtworkBleed(source: player.currentArtwork, drifts: player.isPlaying)
            }
        }
        // A link followed from here has moved the navigator to a page behind
        // the sheet; the sheet gets out of the way of it.
        .onChange(of: nav.current) { dismiss() }
    }

    /// The sleeve, or the words, in the same place — the way a record and its
    /// lyric sheet share a sleeve.
    @ViewBuilder private var stage: some View {
        if ui.showLyrics {
            LyricsPanel()
                .clipShape(.rect(cornerRadius: 12))
                .transition(.opacity)
        } else if let source = player.currentArtwork {
            AlbumArtwork(source: source, size: .tile, cornerRadius: 12)
                .shadow(color: .black.opacity(0.25), radius: 24, y: 12)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .transition(.opacity)
        } else {
            RoundedRectangle(cornerRadius: 12)
                .fill(.quaternary)
                .aspectRatio(1, contentMode: .fit)
                .overlay { Image(systemName: "music.note").font(.largeTitle) }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var titles: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(player.currentEntry?.title ?? "Nothing playing")
                    .font(.title3.weight(.semibold))
                    .lineLimit(1)
                if let entry = player.currentEntry {
                    LinkText(
                        text: entry.artist,
                        target: player.currentArtistId.map { .artist($0) },
                        font: .body
                    )
                    .lineLimit(1)
                    if !entry.album.isEmpty {
                        LinkText(
                            text: entry.album,
                            target: player.currentAlbumId.map { .album($0) },
                            font: .subheadline
                        )
                        .lineLimit(1)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentTransition(.opacity)
            .animation(.easeInOut(duration: 0.2), value: player.currentEntry?.queueItemId)

            if let trackId = player.currentTrackId {
                TrackHeart(trackId: trackId, size: .title3)
            }
        }
    }

    private var transport: some View {
        HStack(spacing: 48) {
            Button { player.previous() } label: {
                Image(systemName: Icon.previous).font(.title)
            }
            Button { player.togglePlayPause() } label: {
                Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                    .font(.system(size: 46))
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 56)
            }
            Button { player.next() } label: {
                Image(systemName: Icon.next).font(.title)
            }
        }
        .buttonStyle(.plain)
        .disabled(player.currentEntry == nil)
    }

    /// What the Mac keeps at the right of its bar: what the output is handed,
    /// radio, and where the sound goes. Lyrics joins them, since there is no
    /// inspector here for it to open in.
    private var extras: some View {
        HStack(spacing: 18) {
            Button {
                ui.toggleLyrics()
            } label: {
                Image(systemName: Icon.lyrics)
                    .symbolVariant(ui.showLyrics ? .fill : .none)
            }
            .accessibilityLabel(ui.showLyrics ? "Show artwork" : "Show lyrics")

            Toggle(isOn: Binding(
                get: { player.radioEnabled },
                set: { player.setRadio($0) }
            )) {
                Image(systemName: Icon.radio)
            }
            .toggleStyle(.button)
            .accessibilityLabel("Radio")

            Spacer()

            if let format = player.currentFormat {
                Text(Format.quality(format))
                    .font(.caption.monospaced())
                    .lineLimit(1)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(.quaternary, in: Capsule())
            }

            RoutePicker()
                .frame(width: 28, height: 28)
        }
        .font(.title3)
        .buttonStyle(.plain)
    }
}

/// The system's output picker — AirPlay, Bluetooth, the speaker. iOS owns the
/// route, so this stands where the Mac's device menu does.
private struct RoutePicker: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let picker = AVRoutePickerView()
        picker.prioritizesVideoDevices = false
        picker.tintColor = .secondaryLabel
        picker.activeTintColor = .label
        return picker
    }

    func updateUIView(_ view: AVRoutePickerView, context: Context) {}
}
