import KoanFFI
import SwiftUI

/// Where a track's bytes are, in one mark.
///
/// A cloud that fills as the track comes down: empty means the server has it
/// and this machine does not, a ring means it is arriving, filled means it is
/// here. One slot rather than two, and the progress lives in it rather than
/// beside it — the eye already goes to this column to ask "have I got this?",
/// and a transfer is the same question mid-answer.
///
/// A local file is not a cloud at all and keeps its own mark. A row that is
/// both — local files that also exist on the server — reads as downloaded,
/// which is what it is.
///
/// Shared by every view that lists tracks so the vocabulary is the same
/// wherever you meet it.
struct SourceBadges: View {
    let onServer: Bool
    let onDisk: Bool
    /// The transfer this row is waiting on, when it is waiting on one.
    ///
    /// An id rather than a figure, deliberately. Reading the figure is what
    /// subscribes a view to a number that moves ten times a second while
    /// anything is downloading — so only the handful of rows actually drawing a
    /// ring do it, and the rest of the list sits still.
    var transferring: String?

    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        Group {
            if let transferring {
                ring(mirror.progress(for: transferring))
            } else if onServer {
                // Visible enough to be read at a glance down a list. At
                // `.quaternary` this was there and effectively invisible, which
                // is the same as not drawing it.
                Image(systemName: onDisk ? "cloud.fill" : "cloud")
                    .foregroundStyle(onDisk ? AnyShapeStyle(.secondary) : AnyShapeStyle(.tertiary))
                    .help(onDisk ? "On your server, downloaded" : "On your server — downloads on play")
            } else if onDisk {
                Image(systemName: "internaldrive")
                    .foregroundStyle(.secondary)
                    .help("Local file")
            }
        }
        .font(.caption2)
        .imageScale(.small)
        // Fixed, so a row does not shift as the mark changes under it — every
        // state has to occupy the same space as every other.
        .frame(width: 14, height: 14)
    }

    /// A transfer whose length the server never gave has no fraction to show
    /// and spins.
    @ViewBuilder
    private func ring(_ fraction: Double?) -> some View {
        if let fraction {
            ProgressView(value: fraction)
                .progressViewStyle(.circular)
                .controlSize(.mini)
                .help("Downloading — \(Int(fraction * 100))%")
        } else {
            ProgressView()
                .progressViewStyle(.circular)
                .controlSize(.mini)
                .help("Downloading")
        }
    }
}

extension SourceBadges {
    /// A library row, plus whatever the queue knows about fetching it. The
    /// queue is the only thing that knows a transfer is running; the library
    /// row only ever learns it finished.
    init(track: Track, queued: QueueItem? = nil) {
        self.init(
            onServer: track.onServer,
            onDisk: track.onDisk,
            transferring: SourceBadges.transfer(of: queued)
        )
    }

    /// The transfer a queue item is waiting on, if its status says it is.
    nonisolated static func transfer(of item: QueueItem?) -> String? {
        guard let item, item.status == .downloading else { return nil }
        return item.queueItemId
    }
}
