import KoanFFI
import SwiftUI

/// ⌘K. The TUI's track/album/artist pickers, as one native sheet.
///
/// Two things carried over deliberately: you can pick *several* things before
/// committing, and committing is three distinct verbs — append, append and
/// play, or replace the queue. A palette that only ever plays one track is a
/// worse tool for building a queue, which is what this app is for.
struct PickerSheet: View {
    @Binding var isPresented: Bool

    @Environment(PlayerModel.self) private var player
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library

    @State private var kind: SearchKind = .track
    @State private var query = ""
    @State private var results: [PickerRow] = []
    @State private var picked: [PickerRow] = []
    @State private var highlighted: PickerRow.ID?
    @State private var searchTask: Task<Void, Never>?
    @State private var resolving = false

    @FocusState private var fieldFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            searchField
            Divider()
            kindPicker
            Divider()
            resultList
                // The commit bar floats over the results rather than being
                // fenced off below them, so the list keeps the full height and
                // you can see what is still under the bar as you scroll.
                .safeAreaInset(edge: .bottom, spacing: 0) { commitBar }
        }
        .frame(width: 660, height: 500)
        .onAppear { fieldFocused = true }
        .onChange(of: query) { _, new in schedule(new) }
        .onChange(of: kind) { _, _ in schedule(query) }
        .onDisappear { searchTask?.cancel() }
    }

    // MARK: - Search

    private var searchField: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
            TextField(prompt, text: $query)
                .textFieldStyle(.plain)
                .font(.title3)
                .focused($fieldFocused)
                .onSubmit { commit(.append) }
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
    }

    private var prompt: String {
        switch kind {
        case .track: "Search tracks"
        case .album: "Search albums"
        case .artist: "Search artists"
        }
    }

    private var kindPicker: some View {
        Picker("", selection: $kind) {
            Text("Tracks").tag(SearchKind.track)
            Text("Albums").tag(SearchKind.album)
            Text("Artists").tag(SearchKind.artist)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    // MARK: - Results

    @ViewBuilder
    private var resultList: some View {
        if query.isEmpty && picked.isEmpty {
            EmptyState(icon: "magnifyingglass", title: "Search your library")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List(selection: $highlighted) {
                if !picked.isEmpty {
                    Section("Selected") {
                        ForEach(picked) { row in
                            PickerRowView(row: row, isPicked: true)
                                .onTapGesture { toggle(row) }
                        }
                    }
                }
                if !results.isEmpty {
                    Section(picked.isEmpty ? "" : "Results") {
                        ForEach(results.filter { r in !picked.contains(where: { $0.id == r.id }) }) { row in
                            PickerRowView(row: row, isPicked: false)
                                .onTapGesture { toggle(row) }
                                .onTapGesture(count: 2) { commitSingle(row) }
                        }
                    }
                }
            }
            .listStyle(.inset)
            .scrollEdgeEffectStyle(.soft, for: .bottom)
        }
    }

    // MARK: - Commit

    private enum Commit {
        case append
        case appendAndPlay
        case replace
    }

    private var commitBar: some View {
        HStack(spacing: 10) {
            if picked.isEmpty {
                Text("Click to select · double-click to play now")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            } else {
                Text("\(picked.count) selected")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button("Clear") { picked = [] }
                    .buttonStyle(.borderless)
                    .font(.caption)
            }

            Spacer()

            if resolving {
                ProgressView().controlSize(.small)
            }

            Button("Replace Queue") { commit(.replace) }
                .keyboardShortcut(.return, modifiers: [.command, .shift])
            Button("Add") { commit(.append) }
                .keyboardShortcut(.return, modifiers: [])
            Button("Add & Play") { commit(.appendAndPlay) }
                .keyboardShortcut(.return, modifiers: .command)
                .buttonStyle(.borderedProminent)
        }
        .disabled(resolving || (picked.isEmpty && highlighted == nil))
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: 20))
        .padding(.horizontal, 12)
        .padding(.bottom, 12)
    }

    private func toggle(_ row: PickerRow) {
        if let index = picked.firstIndex(where: { $0.id == row.id }) {
            picked.remove(at: index)
        } else {
            picked.append(row)
        }
    }

    private func commitSingle(_ row: PickerRow) {
        picked = [row]
        commit(.appendAndPlay)
    }

    /// Albums and artists stand for their tracks, so the selection is expanded
    /// to track IDs before anything reaches the queue.
    private func commit(_ action: Commit) {
        // Nothing ticked means "act on whatever is under the cursor".
        let rows: [PickerRow] = picked.isEmpty
            ? [highlighted.flatMap { id in results.first { $0.id == id } }].compactMap { $0 }
            : picked
        guard !rows.isEmpty else { return }

        resolving = true
        let engine = library.engine
        Task {
            var trackIds: [Int64] = []
            for row in rows {
                switch row.kind {
                case .track:
                    trackIds.append(row.id)
                case .album:
                    trackIds += ((try? await engine.tracks(
                        albumId: row.id, artistId: nil, sort: .album, limit: 500, offset: 0
                    )) ?? []).map(\.id)
                case .artist:
                    trackIds += ((try? await engine.tracks(
                        albumId: nil, artistId: row.id, sort: .album, limit: 2000, offset: 0
                    )) ?? []).map(\.id)
                }
            }

            resolving = false
            guard !trackIds.isEmpty else { return }

            switch action {
            case .append:
                player.enqueue(trackIds: trackIds)
            case .appendAndPlay:
                let existing = player.queue.count
                player.enqueue(trackIds: trackIds)
                // Jump to the first thing just added rather than the queue head.
                if existing > 0 {
                    try? await Task.sleep(for: .milliseconds(120))
                    if player.queue.indices.contains(existing) {
                        player.play(itemId: player.queue[existing].queueItemId)
                    }
                }
            case .replace:
                player.playNow(trackIds: trackIds)
                nav.showQueueWhenReady(watching: player)
            }
            isPresented = false
        }
    }

    // MARK: - Querying

    /// Tracks go through FTS5 and everything else through nucleo. Fuzzy
    /// matching rebuilds its corpus per keystroke, which is fine across a few
    /// thousand artists and wasteful across fifty thousand tracks.
    private func schedule(_ text: String) {
        searchTask?.cancel()
        guard !text.isEmpty else {
            results = []
            return
        }

        let engine = library.engine
        let kind = self.kind
        searchTask = Task {
            try? await Task.sleep(for: .milliseconds(140))
            guard !Task.isCancelled else { return }

            let found: [PickerRow] =
                switch kind {
                case .track:
                    ((try? await engine.search(query: text, limit: 60)) ?? []).map {
                        PickerRow(
                            id: $0.id,
                            kind: .track,
                            title: $0.title,
                            subtitle: "\($0.artistName) — \($0.albumTitle)",
                            durationMs: $0.durationMs
                        )
                    }
                case .album, .artist:
                    ((try? await engine.fuzzySearch(query: text, kind: kind, limit: 40)) ?? []).map {
                        PickerRow(id: $0.id, kind: kind, title: $0.name, subtitle: nil, durationMs: nil)
                    }
                }

            guard !Task.isCancelled else { return }
            results = found
        }
    }
}

struct PickerRow: Identifiable, Equatable, Sendable {
    let id: Int64
    let kind: SearchKind
    let title: String
    let subtitle: String?
    let durationMs: Int64?
}

private struct PickerRowView: View {
    let row: PickerRow
    let isPicked: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: isPicked ? "checkmark.circle.fill" : icon)
                .foregroundStyle(isPicked ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                .frame(width: 16)

            VStack(alignment: .leading, spacing: 1) {
                Text(row.title).lineLimit(1)
                if let subtitle = row.subtitle {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }

            Spacer(minLength: 6)

            if let ms = row.durationMs {
                Text(Format.duration(ms))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.tertiary)
            }
        }
        .contentShape(.rect)
        .padding(.vertical, 2)
    }

    private var icon: String {
        switch row.kind {
        case .track: "music.note"
        case .album: "square.stack"
        case .artist: "music.mic"
        }
    }
}
