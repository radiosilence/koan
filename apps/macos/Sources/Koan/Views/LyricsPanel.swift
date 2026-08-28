import KoanFFI
import SwiftUI

/// Synced lyrics, highlighted against playback position — the TUI's `L` panel.
///
/// The engine hands back already-parsed LRC lines, so this only has to pick the
/// current one and scroll to it.
struct LyricsPanel: View {
    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui

    @State private var lyrics: Lyrics?
    @State private var loadedTrackId: Int64?
    @State private var loading = false
    /// The line being sung. Set by `follow`, which wakes once per line.
    @State private var active: Int?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Lyrics")
                    .font(.headline)
                Spacer()
                if loading {
                    ProgressView().controlSize(.small)
                } else if let source = lyrics?.source {
                    Text(source)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 11)

            Divider()

            content
        }
        .background(.background.secondary)
        .task(id: player.currentTrackId) { await load() }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { ui.lyricsWidth = $0 }
        .onDisappear { ui.lyricsWidth = 0 }
    }

    @ViewBuilder
    private var content: some View {
        if let lyrics, !lyrics.lines.isEmpty {
            synced(lyrics)
        } else if let lyrics, !lyrics.content.isEmpty {
            ScrollView {
                Text(lyrics.content)
                    .font(.callout)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(18)
                    .textSelection(.enabled)
            }
        } else if loading {
            Color.clear
        } else {
            EmptyState(
                icon: "text.quote",
                title: player.currentTrackId == nil ? "Nothing playing" : "No lyrics found"
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func synced(_ lyrics: Lyrics) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 11) {
                    ForEach(Array(lyrics.lines.enumerated()), id: \.offset) { index, line in
                        Text(line.text.isEmpty ? " " : line.text)
                            .font(index == active ? .title3.weight(.semibold) : .callout)
                            .foregroundStyle(index == active ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .id(index)
                            .onTapGesture { player.seek(fraction: fraction(of: line)) }
                    }
                }
                .padding(18)
                .padding(.vertical, 60)
            }
            .onChange(of: active) { _, new in
                guard let new else { return }
                withAnimation(.easeInOut(duration: 0.25)) {
                    proxy.scrollTo(new, anchor: .center)
                }
            }
            // Restarted by the anchor, which moves on a seek, a pause and a
            // track boundary — every reason the line being sung would change
            // other than the song simply carrying on, which `follow` sleeps
            // through by itself.
            .task(id: mirror.playhead) { await follow(lyrics) }
        }
    }

    /// Last line whose timestamp has passed, at `position` seconds.
    private func currentIndex(_ lyrics: Lyrics, at position: Double) -> Int? {
        lyrics.lines.lastIndex { $0.timeSecs <= position }
    }

    /// Follow the song, one line at a time.
    ///
    /// A lyric line changes when a timestamp passes, and the panel knows every
    /// timestamp — so it sleeps until the next one rather than asking where the
    /// playhead is. One wake per line, none at all while paused, and a seek
    /// restarts this because the anchor it reads is what changed.
    private func follow(_ lyrics: Lyrics) async {
        while !Task.isCancelled {
            let playhead = mirror.playhead
            let position = Double(playhead.at()) / 1000
            active = currentIndex(lyrics, at: position)
            guard playhead.playing,
                  let next = lyrics.lines.first(where: { $0.timeSecs > position })
            else { return }
            try? await Task.sleep(for: .seconds(next.timeSecs - position))
        }
    }

    private func fraction(of line: LyricLine) -> Double {
        guard player.durationMs > 0 else { return 0 }
        return (line.timeSecs * 1000 / Double(player.durationMs)).clamped()
    }

    /// Cache first so the panel fills instantly, then LRCLIB in the background
    /// for a miss.
    private func load() async {
        guard let trackId = player.currentTrackId else {
            lyrics = nil
            loadedTrackId = nil
            return
        }
        guard trackId != loadedTrackId else { return }

        loadedTrackId = trackId
        lyrics = try? await library.engine.lyrics(trackId: trackId)
        guard lyrics == nil else { return }

        loading = true
        let engine = library.engine
        let fetched = try? await engine.fetchLyrics(trackId: trackId)
        loading = false
        // The track may have changed while LRCLIB was answering.
        guard loadedTrackId == trackId else { return }
        lyrics = fetched ?? nil
    }
}
