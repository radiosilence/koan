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
    /// 0–1 while this track is being fetched, `nil` when it is not. A transfer
    /// whose length the server never gave has no fraction to show and spins.
    var downloading: Download?

    enum Download {
        case indeterminate
        case fraction(Double)
    }

    var body: some View {
        Group {
            if let downloading {
                ring(downloading)
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

    @ViewBuilder
    private func ring(_ download: Download) -> some View {
        switch download {
        case .fraction(let value):
            ProgressView(value: value)
                .progressViewStyle(.circular)
                .controlSize(.mini)
                .help("Downloading — \(Int(value * 100))%")
        case .indeterminate:
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
            downloading: SourceBadges.download(of: queued)
        )
    }

    /// A queue row, which knows where its bytes came from by its own status.
    init(queued: QueueItem, onServer: Bool, onDisk: Bool) {
        self.init(
            onServer: onServer,
            onDisk: onDisk,
            downloading: SourceBadges.download(of: queued)
        )
    }

    static func download(of item: QueueItem?) -> Download? {
        guard let item, item.status == .downloading else { return nil }
        return item.downloadProgress.map { .fraction($0) } ?? .indeterminate
    }
}
