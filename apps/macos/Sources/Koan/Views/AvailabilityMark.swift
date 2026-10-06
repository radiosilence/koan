#if os(macOS)
import AppKit
import KoanFFI

/// Where a track's file is, or how its download is going — the slot every
/// Mac row that says so draws in, as `SourceBadges` and the queue states draw
/// it in SwiftUI. Right-aligned in its frame, so a column of them lines up.
final class AvailabilityMark: CALayer {
    enum State: Equatable {
        case nothing
        /// Waiting at the front of the download queue.
        case pending
        case failed
        /// Offline, with no file here: not a failure, and drawn apart from one.
        case notHere
        /// Downloading: how far, when the server said how big.
        case transferring(Double?)
        case stored(onServer: Bool, onDisk: Bool)

        var isTransferring: Bool {
            if case .transferring = self { true } else { false }
        }
    }

    private let badge = CALayer()
    private let ring = CAShapeLayer()
    private var badgeImage: CGImage?

    override init() {
        super.init()
        addSublayer(badge)
        addSublayer(ring)
        ring.fillColor = nil
        ring.lineWidth = 1.5
        ring.lineCap = .round
    }

    override init(layer: Any) {
        super.init(layer: layer)
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    /// What a row should say, from what the library and the queue know.
    @MainActor
    static func state(onServer: Bool, onDisk: Bool, queued: QueueItem?, meter: TransferMeter) -> State {
        if let queued, queued.status == .priorityPending { return .pending }
        if let queued, queued.status == .failed { return .failed }
        if let transfer = SourceBadges.transfer(of: queued) { return .transferring(meter.figure(for: transfer)?.progress) }
        return .stored(onServer: onServer, onDisk: onDisk)
    }

    @MainActor
    func show(_ state: State, tint: NSColor, selected: Bool, appearance: NSAppearance) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        ring.isHidden = true
        badge.isHidden = false
        let quiet: NSColor = selected ? .white : .koanTertiaryLabel
        let plain: NSColor = selected ? .white : .koanSecondaryLabel
        switch state {
        case .nothing:
            badgeImage = nil
        case .pending:
            badgeImage = Symbol.image("arrow.down.circle", size: 11, colours: [selected ? .white : tint], appearance: appearance)
        case .failed:
            badgeImage = Symbol.image("exclamationmark.triangle.fill", size: 11, colours: [.systemOrange], appearance: appearance)
        case .notHere:
            badgeImage = Symbol.image("icloud.slash", size: 10, colours: [quiet], appearance: appearance)
        case .transferring(let fraction):
            badge.isHidden = true
            ring.isHidden = false
            appearance.performAsCurrentDrawingAppearance { ring.strokeColor = plain.cgColor }
            if let fraction {
                ring.removeAnimation(forKey: "spin")
                ring.strokeEnd = max(0.02, fraction)
            } else if ring.animation(forKey: "spin") == nil {
                ring.strokeEnd = 0.7
                let spin = CABasicAnimation(keyPath: "transform.rotation.z")
                spin.toValue = Double.pi * 2
                spin.duration = 1
                spin.repeatCount = .infinity
                ring.add(spin, forKey: "spin")
            }
        case .stored(let onServer, let onDisk):
            if onServer {
                badgeImage = Symbol.image(onDisk ? "cloud.fill" : "cloud", size: 9, colours: [onDisk ? plain : quiet], appearance: appearance)
            } else if onDisk {
                badgeImage = Symbol.image("internaldrive", size: 9, colours: [plain], appearance: appearance)
            } else {
                badgeImage = nil
            }
        }
        badge.contents = badgeImage
        setNeedsLayout()
    }

    override func layoutSublayers() {
        super.layoutSublayers()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        // Symbols are drawn at 2×.
        let size = badgeImage.map { CGSize(width: CGFloat($0.width) / 2, height: CGFloat($0.height) / 2) } ?? .zero
        badge.frame = CGRect(x: bounds.maxX - 7 - size.width / 2, y: bounds.midY - size.height / 2, width: size.width, height: size.height)
        ring.frame = CGRect(x: bounds.maxX - 12, y: bounds.midY - 6, width: 12, height: 12)
        ring.path = CGPath(ellipseIn: ring.bounds.insetBy(dx: 1, dy: 1), transform: nil)
        CATransaction.commit()
    }

    /// A frame's figure, between the row's own redraws. Only a ring with a
    /// length to measure against moves; one that is spinning learns it has
    /// one and stops.
    @MainActor
    func take(_ figure: TransferFigure) {
        guard !ring.isHidden, let fraction = figure.progress else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        ring.removeAnimation(forKey: "spin")
        ring.strokeEnd = max(0.02, fraction)
        CATransaction.commit()
    }

    /// What the mark means, for the row's tooltip, where it needs saying.
    static func help(_ state: State, failure: String?) -> String? {
        switch state {
        case .pending: "Queued for download"
        case .failed: failure ?? "Couldn't be fetched"
        case .notHere: "Not on this device"
        default: nil
        }
    }
}
extension AvailabilityMark: TransferGauge {}
#endif
