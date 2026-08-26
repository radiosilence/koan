import KoanFFI
import SwiftUI

/// What koan is fetching from the server, and what it just finished fetching.
///
/// The first place to look when something is not playing: a transfer that has
/// stopped moving says so — its rate falls to nothing while its bar stays put —
/// and a failure keeps its reason rather than only flashing past on the row
/// that caused it.
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

    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            // A record is what you recognise a download by, and this is a list
            // of things you are waiting for.
            AlbumArtwork(source: .track(item.trackId), size: .thumb, cornerRadius: 3)
                .frame(width: 34, height: 34)

            rows
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .contextMenu {
            Button { showInLibrary() } label: {
                Label("Show in Library", systemImage: Icon.album)
            }
            if item.state == .done {
                Button { library.clearDownloads(trackIds: [item.trackId]) } label: {
                    Label("Remove Downloaded File", systemImage: Icon.clear)
                }
            }
        }
    }

    private var rows: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(item.title)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Text(figure)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            // Drawn rather than a `ProgressView`: the stock linear style
            // rendered the same full-width track whatever it was given, which
            // made six transfers at six different percentages look identical.
            //
            // A named colour rather than `.tint`, which a `Canvas` resolves
            // against its own environment and not the view's — hierarchical
            // styles come through, that one comes out invisible.
            Canvas { context, size in
                context.fill(Self.bar(in: size, fraction: 1), with: .style(.quaternary))
                context.fill(Self.bar(in: size, fraction: fraction), with: .color(.koanAccent))
            }
            .frame(height: 4)
            .opacity(isRunning ? 1 : 0.35)

            HStack(spacing: 6) {
                Text(subtitle)
                    .lineLimit(1)
                    .foregroundStyle(
                        item.state == .failed ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary)
                    )
                Spacer(minLength: 8)
                if hovering {
                    Button("Show in Library") { showInLibrary() }
                        .buttonStyle(.link)
                        .font(.caption)
                }
            }
            .font(.caption)
        }
    }

    /// The record it came off, which is where you go to find it. Resolved when
    /// asked rather than carried on every row — the store holds transfers, not
    /// library rows, and most rows are never clicked.
    private func showInLibrary() {
        let engine = library.engine
        let trackId = item.trackId
        Task {
            guard let albumId = (try? await engine.track(trackId: trackId))??.albumId else {
                return
            }
            nav.open(album: albumId, highlighting: trackId)
        }
    }

    private var isRunning: Bool { item.state == .running || item.state == .queued }

    /// What has arrived. A transfer with no stated length has no fraction, and
    /// draws an empty bar rather than a full one — it is going, not finished.
    private var fraction: Double {
        if item.state == .done { return 1 }
        return item.progress ?? 0
    }

    private var subtitle: String {
        switch item.state {
        case .failed: item.failureReason ?? "Couldn't be fetched"
        case .queued: item.artist.isEmpty ? "Waiting" : "\(item.artist) — waiting"
        case .done: item.artist.isEmpty ? "Downloaded" : "\(item.artist) — downloaded"
        case .running: rateAndSize
        }
    }

    /// "Artist · 4.2 MB/s · 210 MB of 451 MB", dropping whatever is not known.
    /// A rate of nothing is a stall, and saying so is the point of the row.
    private var rateAndSize: String {
        var parts: [String] = []
        if !item.artist.isEmpty { parts.append(item.artist) }
        parts.append(
            item.bytesPerSecond > 0
                ? "\(Format.bytes(Int64(item.bytesPerSecond)))/s"
                : "stalled"
        )
        if item.totalBytes > 0 {
            parts.append(
                "\(Format.bytes(Int64(item.bytesWritten))) of \(Format.bytes(Int64(item.totalBytes)))"
            )
        } else if item.bytesWritten > 0 {
            parts.append(Format.bytes(Int64(item.bytesWritten)))
        }
        return parts.joined(separator: " · ")
    }

    private var figure: String {
        switch item.state {
        case .done: "Done"
        case .failed: "Failed"
        case .queued: "Queued"
        case .running: item.progress.map { "\(Int($0 * 100))%" } ?? ""
        }
    }

    private static func bar(in size: CGSize, fraction: Double) -> Path {
        let width = size.width * min(1, max(0, fraction))
        guard width > 0 else { return Path() }
        return Capsule().path(in: CGRect(x: 0, y: 0, width: width, height: size.height))
    }
}
