import KoanFFI
import SwiftUI

/// What koan is fetching from the server, and what it just finished fetching.
///
/// Mostly a page for when something looks wrong: a transfer that is not moving
/// says so here, with the figure it is stuck on, and a failure keeps its reason
/// rather than only flashing past on the row that caused it.
struct DownloadsView: View {
    @Environment(DownloadsModel.self) private var downloads

    var body: some View {
        Group {
            if downloads.items.isEmpty {
                ContentUnavailableView(
                    "Nothing downloading",
                    systemImage: Icon.downloads,
                    description: Text(
                        "Tracks fetched from your server appear here while they arrive."
                    )
                )
            } else {
                List {
                    ForEach(downloads.items, id: \.queueItemId) { item in
                        DownloadRow(item: item)
                            .listRowSeparator(.hidden)
                    }
                }
                .listStyle(.inset)
            }
        }
        .navigationTitle("Downloads")
        .toolbar {
            if downloads.hasSettled {
                Button { downloads.clearSettled() } label: {
                    Label("Clear Finished", systemImage: Icon.clear)
                }
                .help("Forget the transfers that have already settled")
            }
        }
        .task { downloads.reload() }
    }
}

private struct DownloadRow: View {
    let item: DownloadEntry

    var body: some View {
        HStack(spacing: 12) {
            SourceBadges(onServer: true, onDisk: item.state == .done, downloading: badge)

            VStack(alignment: .leading, spacing: 3) {
                Text(item.title).lineLimit(1)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(
                        item.state == .failed ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary)
                    )
                    .lineLimit(1)
                // Only while it is moving. A finished transfer's bar is a full
                // one, which says nothing the filled cloud has not — and a
                // transfer with no stated length has no fraction to draw, where
                // a bar sitting at zero would read as stalled.
                if isRunning, let progress = item.progress {
                    ProgressView(value: progress, total: 1)
                        .progressViewStyle(.linear)
                }
            }

            Spacer(minLength: 8)

            Text(figure)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 3)
    }

    private var isRunning: Bool { item.state == .running || item.state == .queued }

    private var badge: SourceBadges.Download? {
        guard isRunning else { return nil }
        return item.progress.map { .fraction($0) } ?? .indeterminate
    }

    private var subtitle: String {
        switch item.state {
        case .failed: item.failureReason ?? "Couldn't be fetched"
        case .queued: item.artist.isEmpty ? "Waiting" : "\(item.artist) — waiting"
        case .done: item.artist.isEmpty ? "Downloaded" : "\(item.artist) — downloaded"
        case .running: item.artist
        }
    }

    /// A percentage where there is one, and bytes where there is not — a
    /// transfer the server gave no length for is still visibly moving.
    private var figure: String {
        guard isRunning else { return "" }
        if let progress = item.progress { return "\(Int(progress * 100))%" }
        return Format.bytes(Int64(item.bytesWritten))
    }
}
