import KoanFFI
import SwiftUI

/// What koan is fetching from the server, and what it just finished fetching.
///
/// The first place to look when something is not playing: a transfer that has
/// stopped moving says so — its rate falls to nothing while its bar stays put —
/// and a failure keeps its reason rather than only flashing past on the row
/// that caused it.
struct DownloadsView: View {
    @Environment(EngineMirror.self) private var mirror
    @Environment(AppState.self) private var app

    var body: some View {
        Group {
            if mirror.transfers.isEmpty {
                ContentUnavailableView(
                    "Nothing downloading",
                    systemImage: Icon.downloads,
                    description: Text(
                        "Tracks fetched from your server appear here while they arrive."
                    )
                )
            } else {
                List {
                    ForEach(mirror.transfers, id: \.queueItemId) { transfer in
                        DownloadRow(transfer: transfer)
                    }
                }
                .listStyle(.inset)
            }
        }
        .navigationTitle("Downloads")
        .toolbar {
            if mirror.hasSettledTransfers {
                Button { app.engine.clearSettledDownloads() } label: {
                    Label("Clear Finished", systemImage: Icon.clear)
                }
                .help("Forget the transfers that have already settled")
            }
        }
    }
}

private struct DownloadRow: View {
    let transfer: Transfer

    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @State private var hovering = false

    /// The numbers, read here rather than carried on the row. They move ten
    /// times a second while this row is going and not at all once it has
    /// settled — which is exactly what the two slices are for.
    private var figures: TransferFigure? { mirror.figure(for: transfer.queueItemId) }
    private var bytesWritten: UInt64 { figures?.bytesWritten ?? 0 }
    private var totalBytes: UInt64 { figures?.totalBytes ?? 0 }
    private var bytesPerSecond: UInt64 { figures?.bytesPerSecond ?? 0 }
    private var progress: Double? { figures?.progress }

    var body: some View {
        HStack(spacing: 10) {
            // A record is what you recognise a download by, and this is a list
            // of things you are waiting for.
            AlbumArtwork(source: .track(transfer.trackId), size: .thumb, cornerRadius: 3)
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
            if transfer.state == .done {
                Button { library.clearDownloads(trackIds: [transfer.trackId]) } label: {
                    Label("Remove Downloaded File", systemImage: Icon.clear)
                }
            }
        }
    }

    private var rows: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(transfer.title)
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
            // Hierarchical styles, which a `Canvas` resolves against its own
            // environment — `.tint` does not come through it and drew nothing.
            // What has arrived reads as present rather than as coloured: this
            // is a quantity, not a state, and the accent said neither.
            Canvas { context, size in
                context.fill(Self.bar(in: size, fraction: 1), with: .style(.quaternary))
                context.fill(Self.bar(in: size, fraction: fraction), with: .style(.primary))
            }
            .frame(height: 4)
            .opacity(isRunning ? 1 : 0.35)

            HStack(spacing: 6) {
                Text(subtitle)
                    .lineLimit(1)
                    .foregroundStyle(
                        transfer.state == .failed ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary)
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
        let trackId = transfer.trackId
        Task {
            guard let albumId = (try? await engine.track(trackId: trackId))??.albumId else {
                return
            }
            nav.open(album: albumId, highlighting: trackId)
        }
    }

    private var isRunning: Bool { transfer.state == .running || transfer.state == .queued }

    /// What has arrived. A transfer with no stated length has no fraction, and
    /// draws an empty bar rather than a full one — it is going, not finished.
    private var fraction: Double {
        if transfer.state == .done { return 1 }
        return progress ?? 0
    }

    private var subtitle: String {
        switch transfer.state {
        case .failed: transfer.failureReason ?? "Couldn't be fetched"
        case .queued: transfer.artist.isEmpty ? "Waiting" : "\(transfer.artist) — waiting"
        case .done: transfer.artist.isEmpty ? "Downloaded" : "\(transfer.artist) — downloaded"
        case .running: rateAndSize
        }
    }

    /// "Artist · 4.2 MB/s · 210 MB of 451 MB", dropping whatever is not known.
    /// A rate of nothing is a stall, and saying so is the point of the row.
    private var rateAndSize: String {
        var parts: [String] = []
        if !transfer.artist.isEmpty { parts.append(transfer.artist) }
        parts.append(
            bytesPerSecond > 0
                ? "\(Format.bytes(Int64(bytesPerSecond)))/s"
                : "stalled"
        )
        if totalBytes > 0 {
            parts.append(
                "\(Format.bytes(Int64(bytesWritten))) of \(Format.bytes(Int64(totalBytes)))"
            )
        } else if bytesWritten > 0 {
            parts.append(Format.bytes(Int64(bytesWritten)))
        }
        return parts.joined(separator: " · ")
    }

    private var figure: String {
        switch transfer.state {
        case .done: "Done"
        case .failed: "Failed"
        case .queued: "Queued"
        case .running: progress.map { "\(Int($0 * 100))%" } ?? ""
        }
    }

    private static func bar(in size: CGSize, fraction: Double) -> Path {
        let width = size.width * min(1, max(0, fraction))
        guard width > 0 else { return Path() }
        return Capsule().path(in: CGRect(x: 0, y: 0, width: width, height: size.height))
    }
}
