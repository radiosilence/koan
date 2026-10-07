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
    @Environment(AppState.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var showingDevices = false
    @State private var showingControl = false

    var body: some View {
        VStack(spacing: 20) {
            stage
                .padding(.horizontal, 28)
                .padding(.top, 36)
                .frame(maxHeight: .infinity)

            titles.padding(.horizontal, 28)
            SeekBar().padding(.horizontal, 20)
            transport
            toggles
                .padding(.horizontal, 28)
            choices
                .padding(.horizontal, 20)
                .padding(.bottom, 12)
        }
        .sheetGrabber()
        // The playing record's own wash, whatever page the sheet was opened
        // over — this is the one screen that is only about that record.
        .presentationBackground {
            ZStack {
                Rectangle().fill(KoanTheme.ground(.background))
                ArtworkBleed(source: player.currentArtwork, drifts: player.isPlaying)
            }
        }
        // A link followed from here has moved the navigator to a page behind
        // the sheet; the sheet gets out of the way of it.
        .onChange(of: nav.current) { dismiss() }
        .outputSheet(isPresented: $showingDevices)
        .controlSheet(isPresented: $showingControl)
    }

    /// The sleeve, or the words, in the same place — the way a record and its
    /// lyric sheet share a sleeve.
    @ViewBuilder private var stage: some View {
        if ui.showLyrics {
            // The words stand in the wash, as the sleeve did.
            LyricsPanel()
                .transition(.opacity)
        } else if let source = player.currentArtwork {
            AlbumArtwork(source: source, size: .tile, cornerRadius: KoanTheme.radius(12))
                .koanShadow(0.25, radius: 24, y: 12)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .transition(.opacity)
        } else {
            RoundedRectangle(cornerRadius: KoanTheme.radius(12))
                .fill(.quaternary)
                .aspectRatio(1, contentMode: .fit)
                .overlay { Image(systemName: "music.note").font(.role(.display, system: .largeTitle)) }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var titles: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(player.resolving ?? player.currentEntry?.title ?? KoanTheme.label("Nothing playing"))
                    .font(.role(.titleSmall, system: .title3.weight(.semibold)))
                    .foregroundStyle(KoanTheme.style(.strong, system: .primary))
                    .lineLimit(1)
                if player.resolving != nil {
                    Text("Loading…").koanCase()
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                } else if let entry = player.currentEntry {
                    LinkText(
                        text: entry.artist,
                        target: player.currentArtistId.map { .artist($0) },
                        font: .role(.body, system: .body)
                    )
                    .lineLimit(1)
                    if !entry.album.isEmpty {
                        LinkText(
                            text: entry.album,
                            target: player.currentAlbumId.map { .album($0) },
                            font: .role(.meta, system: .subheadline)
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

    /// Shuffle and repeat flank the three that move, at the size of the
    /// toggles below: they set how the queue plays rather than moving it.
    private var transport: some View {
        HStack(spacing: 0) {
            ShuffleButton().font(.role(.titleSmall, system: .title3))
            Spacer()
            Button { player.previous() } label: {
                Image(systemName: Icon.previous).font(.role(.title, system: .title))
            }
            Spacer()
            Button { player.togglePlayPause() } label: {
                Group {
                    if player.isWaitingForTrack {
                        ProgressView().controlSize(.large)
                    } else {
                        Image(systemName: player.isPlaying ? "pause.fill" : Icon.play)
                            .font(.system(size: KoanTheme.metric(24, system: 46)))
                            .contentTransition(.symbolEffect(.replace))
                    }
                }
                .frame(width: 56, height: 56)
            }
            .koanButtons(.iconOutlined)
            .accessibilityLabel(player.isWaitingForTrack ? "Loading" : player.isPlaying ? "Pause" : "Play")
            Spacer()
            Button { player.next() } label: {
                Image(systemName: Icon.next).font(.role(.title, system: .title))
            }
            Spacer()
            RepeatButton().font(.role(.titleSmall, system: .title3))
        }
        .padding(.horizontal, 28)
        .buttonStyle(.plain)
        .disabled(player.currentEntry == nil)
    }

    /// What is switched on or off for listening, and what the output is
    /// handed: the lyrics, the sleep timer, the format, and AirPlay, which iOS
    /// owns.
    private var toggles: some View {
        HStack(spacing: 0) {
            Button {
                ui.toggleLyrics()
            } label: {
                Image(systemName: Icon.lyrics)
                    .symbolVariant(ui.showLyrics ? .fill : .none)
            }
            .accessibilityLabel(ui.showLyrics ? "Show artwork" : "Show lyrics")

            Spacer(minLength: 12)
            SleepButton()
                .font(.role(.meta, system: .subheadline))

            if let format = player.currentFormat {
                Spacer(minLength: 12)
                Text(Format.quality(format))
                    .lineLimit(1)
                    .koanBadge()
            }

            // This phone's own route; nothing it chooses reaches another device.
            if !player.isControllingAnother {
                Spacer(minLength: 12)
                RoutePicker()
                    .frame(width: 28, height: 28)
            }
        }
        .font(.role(.titleSmall, system: .title3))
        .buttonStyle(.plain)
    }

    /// Which device plays, through what, and with which preset: one pill
    /// each, named in full. Spread evenly at their own widths where they fit;
    /// where they do not, the short ones keep their width and the long one is
    /// shortened. Every sheet and menu they open names its choices in full.
    private var choices: some View {
        ViewThatFits(in: .horizontal) {
            choiceRow(natural: true)
            choiceRow(natural: false)
        }
        .buttonStyle(.plain)
    }

    private func choiceRow(natural: Bool) -> some View {
        let device = player.isControllingAnother
            ? player.controlled?.name ?? "Another device"
            : "This \(UIDevice.current.model)"
        let outputName = player.outputName ?? "Output"
        return HStack(spacing: natural ? 0 : 8) {
            if natural { Spacer(minLength: 0) }
            if player.hasOtherDevices || player.isControllingAnother {
                Button { showingControl = true } label: {
                    // This phone in control is the usual case, and its glyph
                    // says enough; the room goes to the preset's name.
                    Pill(
                        systemImage: Action.control.glyph,
                        text: player.isControllingAnother ? device : nil,
                        tinted: player.isControllingAnother
                    )
                }
                .accessibilityLabel(player.isControllingAnother ? "Controlling \(device)" : device)
                .accessibilityHint(player.isControllingAnother ? "" : "Control another kōan")
                .pillWidth(natural, name: player.isControllingAnother ? device : "")
                if natural { Spacer(minLength: 8) }
            }
            if player.canChooseOutput {
                Button { showingDevices = true } label: {
                    Pill(systemImage: "hifispeaker", text: outputName, tinted: player.renderer != nil)
                }
                .accessibilityLabel("Output: \(player.outputName ?? "default")")
                .pillWidth(natural, name: outputName)
            }
            // This phone's own output's preset; another device's is chosen in
            // Output.
            if !player.isControllingAnother, let output,
               let presets = Presets(dsp: app.dsp, device: output.device) {
                if natural { Spacer(minLength: 8) }
                let preset = presets.summary
                PresetMenu(presets: presets, title: output.name) {
                    Pill(
                        systemImage: "slider.horizontal.3",
                        text: preset,
                        tinted: player.currentFormat?.dsp != nil
                    )
                }
                .accessibilityLabel("Preset: \(preset)")
                .pillWidth(natural, name: preset)
            }
            if natural { Spacer(minLength: 0) }
        }
    }

    /// What the music is coming out of, as profiles name it: a renderer the
    /// phone plays to, by its UDN, or else the route. Its preset is the one
    /// that is heard, so it is the one shown and changed.
    private var output: (device: String, name: String)? {
        if let renderer = player.renderer {
            return (renderer.udn, renderer.name)
        }
        return app.dsp.route.map { ($0, $0) }
    }
}

/// A device choice under the transport: an icon and a name, tinted while it
/// is not this phone's own way of playing.
private struct Pill: View {
    let systemImage: String
    let text: String?
    let tinted: Bool

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: systemImage)
                .foregroundStyle(KoanTheme.style(tinted ? .accent : .muted, system: tinted ? AnyShapeStyle(.tint) : KoanTheme.style(.muted, system: .secondary)))
            if let text {
                Text(text)
                    .lineLimit(1)
                    .foregroundStyle(KoanTheme.style(.ink, system: .primary))
            }
        }
        .font(.role(.meta, system: .subheadline))
        .koanChip()
    }
}

private extension View {
    /// At its own width, or, where the row is short, sized shortest name
    /// first: a stack shares out what is left evenly among views that can all
    /// shrink, which would cut a short name for the sake of a long one.
    @ViewBuilder func pillWidth(_ natural: Bool, name: String) -> some View {
        if natural {
            fixedSize()
        } else {
            layoutPriority(-Double(name.count))
        }
    }
}

/// The system's output picker — AirPlay, Bluetooth, the speaker. iOS owns the
/// route, so this stands where the Mac's device menu does.
struct RoutePicker: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let picker = AVRoutePickerView()
        picker.prioritizesVideoDevices = false
        picker.tintColor = .koanSecondaryLabel
        picker.activeTintColor = .koanLabel
        return picker
    }

    func updateUIView(_ view: AVRoutePickerView, context: Context) {}
}

/// How Now Playing opens. A sheet, as the platform has it, except on an iPad in
/// the theme, where a sheet is a rounded card floating over the page: there it
/// covers the screen, and goes with a swipe down or its chevron.
struct NowPlayingPresentation: ViewModifier {
    @Binding var isPresented: Bool
    @Environment(\.horizontalSizeClass) private var width

    func body(content: Content) -> some View {
        #if os(iOS)
        if KoanTheme.isOn && width == .regular {
            content.fullScreenCover(isPresented: $isPresented) {
                NowPlayingSheet()
                    .overlay(alignment: .topLeading) {
                        Button { isPresented = false } label: {
                            KoanIcon("chevron.down")
                                .font(.koan(.titleSmall))
                                .frame(width: 44, height: 44)
                                .contentShape(Rectangle())
                        }
                        .koanButton(.icon)
                        .accessibilityLabel("Close")
                        .padding(KoanTheme.Space.m)
                    }
                    .gesture(
                        DragGesture(minimumDistance: 24).onEnded { drag in
                            let down = drag.translation
                            if down.height > 120, down.height > abs(down.width) * 2 { isPresented = false }
                        }
                    )
            }
        } else {
            sheet(content)
        }
        #else
        sheet(content)
        #endif
    }

    private func sheet(_ content: Content) -> some View {
        content.sheet(isPresented: $isPresented) {
            NowPlayingSheet()
                .presentationDetents([.large])
        }
    }
}
