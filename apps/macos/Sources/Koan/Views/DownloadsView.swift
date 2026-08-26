import KoanFFI
import SwiftUI

/// What koan is fetching from the server, and what it just finished fetching.
///
/// Mostly a page for when something looks wrong: a transfer that is not moving
/// says so here, with the figure it is stuck on, and a failure keeps its reason
/// rather than only flashing past on the row that caused it.
struct DownloadsView: View {
    @Environment(DownloadsModel.self) private var downloads
    @Environment(Navigator.self) private var nav

    var body: some View {
        Group {
            if downloads.items.isEmpty {
                ContentUnavailableView(
                    "Nothing downloading",
                    systemImage: "cloud",
                    description: Text("Tracks fetched from your server appear here while they arrive.")
                )
            } else {
                List {
                    ForEach(downloads.items) { item in
                        DownloadRow(item: item)
                            .listRowSeparator(.hidden)
                    }
                }
                .listStyle(.inset)
            }
        }
        .navigationTitle("Downloads")
        .toolbar {
            if downloads.items.contains(where: { !$0.isActive }) {
                Button { downloads.clearSettled() } label: {
                    Label("Clear Finished", systemImage: Icon.clear)
                }
                .help("Forget the transfers that have already settled")
            }
        }
    }
}

private struct DownloadRow: View {
    let item: DownloadsModel.Item

    var body: some View {
        HStack(spacing: 12) {
            SourceBadges(
                onServer: true,
                onDisk: item.state == .finished,
                downloading: badge
            )

            VStack(alignment: .leading, spacing: 3) {
                Text(item.title).lineLimit(1)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(item.state.isFailure ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                    .lineLimit(1)
                // Only while it is moving. A finished transfer's bar is a full
                // one, which says nothing the filled cloud has not.
                if item.isActive {
                    ProgressView(value: item.progress ?? 0, total: 1)
                        .progressViewStyle(.linear)
                        // An unknown length has no fraction to draw, and a bar
                        // sitting at zero reads as stalled rather than unmeasured.
                        .opacity(item.progress == nil ? 0 : 1)
                }
            }

            Spacer(minLength: 8)

            if let progress = item.progress, item.isActive {
                Text("\(Int(progress * 100))%")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 3)
    }

    private var badge: SourceBadges.Download? {
        guard item.isActive else { return nil }
        return item.progress.map { .fraction($0) } ?? .indeterminate
    }

    private var subtitle: String {
        switch item.state {
        case .running: item.artist
        case .finished: item.artist.isEmpty ? "Downloaded" : "\(item.artist) — downloaded"
        case .failed(let reason): reason
        }
    }
}

private extension DownloadsModel.State {
    var isFailure: Bool {
        if case .failed = self { true } else { false }
    }
}
