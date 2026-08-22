import KoanFFI
import SwiftUI

/// Synced lyrics, highlighted against playback position — the TUI's `L` panel.
///
/// The engine hands back already-parsed LRC lines, so this only has to pick the
/// current one and scroll to it.
struct LyricsPanel: View {
    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    @State private var lyrics: Lyrics?
    @State private var loadedTrackId: Int64?
    @State private var loading = false

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
        let active = currentIndex(lyrics)
        return ScrollViewReader { proxy in
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
        }
    }

    /// Last line whose timestamp has passed.
    private func currentIndex(_ lyrics: Lyrics) -> Int? {
        let position = Double(player.nowPlaying.positionMs) / 1000
        return lyrics.lines.lastIndex { $0.timeSecs <= position }
    }

    private func fraction(of line: LyricLine) -> Double {
        guard player.nowPlaying.durationMs > 0 else { return 0 }
        return (line.timeSecs * 1000 / Double(player.nowPlaying.durationMs)).clamped()
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
        lyrics = try? library.engine.lyrics(trackId: trackId)
        guard lyrics == nil else { return }

        loading = true
        let engine = library.engine
        let fetched = await Task.detached(priority: .utility) {
            try? engine.fetchLyrics(trackId: trackId)
        }.value
        loading = false
        // The track may have changed while LRCLIB was answering.
        guard loadedTrackId == trackId else { return }
        lyrics = fetched ?? nil
    }
}
