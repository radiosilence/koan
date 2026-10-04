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
    /// An id rather than a figure, deliberately. The figure moves at the
    /// display's rate while the transfer runs, and `TransferMeter` hands it to
    /// the ring's layer; no view body reads it.
    var transferring: String?

    @Environment(TransferMeter.self) private var meter

    var body: some View {
        Group {
            if let transferring {
                TransferRing(transfer: transferring, meter: meter)
                    .help("Downloading")
            } else if onServer {
                // Visible enough to be read at a glance down a list.
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
}

/// The ring a transfer draws, fed by `TransferMeter` as layer geometry.
private struct TransferRing: PlatformViewRepresentable {
    let transfer: String
    let meter: TransferMeter

    typealias PlatformViewType = TransferRingView

    func makeView(context: Context) -> TransferRingView { TransferRingView() }

    func updateView(_ view: TransferRingView, context: Context) {
        view.meter = meter
        meter.follow(view, transfer: transfer)
    }

    static func dismantleView(_ view: TransferRingView, coordinator: ()) {
        view.meter?.follow(view, transfer: nil)
    }
}

/// A ring that fills as the bytes land. A transfer whose length the server
/// never gave has no fraction to show and spins.
final class TransferRingView: LayerView, TransferGauge {
    private let ring = CAShapeLayer()
    weak var meter: TransferMeter?

    override init(frame: CGRect) {
        super.init(frame: frame)
        ring.fillColor = nil
        ring.lineWidth = 1.5
        ring.lineCap = .round
        ring.strokeEnd = 0.7
        ring.actions = ["strokeEnd": NSNull(), "path": NSNull(), "bounds": NSNull(), "position": NSNull()]
        hostLayer.addSublayer(ring)
        appearanceChanged()
        spin()
    }

    func take(_ figure: TransferFigure) {
        guard let fraction = figure.progress else { return spin() }
        ring.removeAnimation(forKey: "spin")
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        ring.strokeEnd = max(0.02, fraction)
        CATransaction.commit()
    }

    private func spin() {
        guard ring.animation(forKey: "spin") == nil else { return }
        ring.strokeEnd = 0.7
        let spin = CABasicAnimation(keyPath: "transform.rotation.z")
        spin.toValue = Double.pi * 2
        spin.duration = 1
        spin.repeatCount = .infinity
        ring.add(spin, forKey: "spin")
    }

    override func layoutLayers() {
        let side = min(bounds.width, bounds.height, 12)
        ring.frame = CGRect(x: (bounds.width - side) / 2, y: (bounds.height - side) / 2, width: side, height: side)
        ring.path = CGPath(ellipseIn: ring.bounds.insetBy(dx: 1, dy: 1), transform: nil)
    }

    override func appearanceChanged() { ring.strokeColor = resolved(.secondaryLabel) }
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
