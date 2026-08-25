import Foundation
import KoanFFI
import SwiftUI

/// What you have listened to, most recent first.
///
/// A list of events, not of tracks: a record you played three times is three
/// rows. That is the whole point of it, so nothing here deduplicates or
/// regroups by album the way the queue does — the only grouping is the day,
/// which is how people actually reach for this ("what was that thing on
/// Tuesday").
///
/// Read-only. Rows link through to the album and the artist and offer the
/// usual play/queue menu, but there is nothing here to reorder or remove.
struct HistoryView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(Navigator.self) private var nav
    @Environment(PlayerModel.self) private var player
    @State private var selection: Set<Int64> = []
    @State private var confirmingClear = false

    private var entries: [PlayHistoryEntry] { library.visiblePlayHistory }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 16)

            if entries.isEmpty {
                EmptyState(
                    icon: "clock.arrow.circlepath",
                    title: library.filter.isEmpty ? "Nothing played yet" : "No matches",
                    detail: library.filter.isEmpty
                        ? "Everything you play lands here, most recent first." : nil
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(selection: $selection) {
                    ForEach(days, id: \.key) { day in
                        Section(day.key) {
                            ForEach(day.entries, id: \.id) { entry in
                                HistoryRow(entry: entry)
                                    .tag(entry.id)
                            }
                        }
                    }
                }
                .listStyle(.inset)
                .clearsSelection($selection)
                .contextMenu(forSelectionType: Int64.self) { ids in
                    menu(for: ids)
                } primaryAction: { ids in
                    play(ids)
                }
                .onKeyPress(.return) {
                    play(selection)
                    return .handled
                }
                .onDeleteCommand { forgetSelected() }
            }
        }
        .alert("Clear History?", isPresented: $confirmingClear) {
            Button("Cancel", role: .cancel) {}
            Button("Clear", role: .destructive) { library.clearPlayHistory() }
        } message: {
            Text("Every play is forgotten. This cannot be undone.")
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("History")
                .font(.system(size: 26, weight: .semibold))
            Text(entries.count == 1 ? "1 play" : "\(entries.count) plays")
                .font(.callout)
                .foregroundStyle(.secondary)
            Spacer(minLength: 0)
            Button("Clear…") { confirmingClear = true }
                .disabled(library.playHistory.isEmpty)
        }
    }

    /// Selection is by entry id, not track id — the same track can
    /// legitimately appear many times and each row stands on its own.
    private func tracks(for ids: Set<Int64>) -> [Track] {
        entries.filter { ids.contains($0.id) }.map(\.track)
    }

    /// Plays the selection, keeping the rest of that run behind it.
    private func play(_ ids: Set<Int64>) {
        let chosen = tracks(for: ids)
        guard !chosen.isEmpty else { return }
        player.playNow(trackIds: chosen.map(\.id))
        nav.showQueueWhenReady(watching: player)
    }

    /// Forgets the selected plays. The tracks themselves are untouched —
    /// history is a log, and this only erases the log.
    private func forgetSelected() {
        guard !selection.isEmpty else { return }
        library.forgetPlays(ids: selection)
        selection = []
    }

    /// The rows under the pointer, as a menu.
    @ViewBuilder
    private func menu(for ids: Set<Int64>) -> some View {
        let chosen = tracks(for: ids)
        if chosen.count == 1, let track = chosen.first {
            PlayableMenu(playable: .track(track))
        } else if !chosen.isEmpty {
            let trackIds = chosen.map(\.id)
            Button("Play") {
                player.playNow(trackIds: trackIds)
                nav.showQueueWhenReady(watching: player)
            }
            Button("Play Next") { player.playNext(trackIds: trackIds) }
            Button("Add to Queue") { player.enqueue(trackIds: trackIds) }
        }
        if !chosen.isEmpty {
            Divider()
            Button("Remove from History") { library.forgetPlays(ids: ids) }
        }
    }

    // MARK: - Day grouping

    private struct Day {
        let key: String
        var entries: [PlayHistoryEntry]
    }

    /// Runs of consecutive entries that fall on the same day. The list is
    /// already ordered by time, so a single pass is enough — no sorting, and
    /// two visits to the same day cannot be separated.
    private var days: [Day] {
        var out: [Day] = []
        for entry in entries {
            let key = HistoryDate.day(entry.playedAt)
            if out.last?.key == key {
                out[out.count - 1].entries.append(entry)
            } else {
                out.append(Day(key: key, entries: [entry]))
            }
        }
        return out
    }
}

private struct HistoryRow: View {
    let entry: PlayHistoryEntry

    @State private var hovering = false

    private var track: Track { entry.track }

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if hovering {
                    RowPlayButton(playable: .track(track), visible: true)
                } else {
                    Text(HistoryDate.time(entry.playedAt))
                        .foregroundStyle(.tertiary)
                }
            }
            .font(.caption.monospacedDigit())
            .frame(width: 46, alignment: .trailing)

            // The cover is what you recognise a record by, and scanning back
            // through a week of listening is exactly that job.
            Group {
                if let albumId = track.albumId {
                    AlbumArtwork(source: .album(albumId), cornerRadius: 3)
                } else {
                    Image(systemName: "music.note")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            .frame(width: 34, height: 34)

            VStack(alignment: .leading, spacing: 1) {
                Text(track.title)
                    .lineLimit(1)
                HStack(spacing: 5) {
                    LinkText(
                        text: track.artistName,
                        target: track.artistId.map { .artist($0) },
                        font: .caption
                    )
                    Text("·")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                    LinkText(
                        text: track.albumTitle,
                        target: track.albumId.map { .album($0) },
                        font: .caption
                    )
                }
            }

            Spacer(minLength: 8)

            // A play recorded by another client scrobbling in did not happen
            // here, and saying so stops it reading as a phantom.
            if entry.source != "local" {
                Image(systemName: "arrow.down.circle")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .help("Scrobbled by another client")
            }

            Text(Format.duration(track.durationMs))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 48, alignment: .trailing)
        }
        .frame(height: 44)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }
}

/// Day and time labels for history rows, with the formatters built once —
/// `DateFormatter` is expensive and a list rebuild would make hundreds.
private enum HistoryDate {
    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.doesRelativeDateFormatting = true  // gives "Today" / "Yesterday"
        f.dateStyle = .full
        f.timeStyle = .none
        return f
    }()

    private static let timeFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .none
        f.timeStyle = .short
        return f
    }()

    static func day(_ unixSeconds: Int64) -> String {
        dayFormatter.string(from: Date(timeIntervalSince1970: TimeInterval(unixSeconds)))
    }

    static func time(_ unixSeconds: Int64) -> String {
        timeFormatter.string(from: Date(timeIntervalSince1970: TimeInterval(unixSeconds)))
    }
}
