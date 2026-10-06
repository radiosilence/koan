#if canImport(AppKit)
import AppKit
#else
import SwiftUI
import UIKit
#endif
import KoanFFI
import QuartzCore

/// How much of a record is on this device, along the foot of its sleeve, and
/// whether more of it is arriving. Partly here and still, it is a muted bar.
/// While any of its tracks download it takes the tint and advances with their
/// bytes, which `TransferMeter` hands it at the display's rate; when the last
/// one settles it is still again.
final class DownloadBarLayer: CALayer {
    private let fill = CALayer()
    private var onDevice: AlbumOnDevice?
    /// The in-flight tracks' progress, summed: how many tracks' worth of
    /// bytes have arrived for tracks not yet counted as here.
    private var arriving: Double = 0

    override init() {
        super.init()
        cornerRadius = KoanTheme.radius(1.5)
        fill.cornerRadius = KoanTheme.radius(1.5)
        addSublayer(fill)
        isHidden = true
    }

    override init(layer: Any) {
        super.init(layer: layer)
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    /// `downloading` is whether any of the record's tracks are in the download
    /// store and not yet settled.
    @MainActor
    func show(_ onDevice: AlbumOnDevice?, downloading: Bool, tint: CGColor, muted: CGColor) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        self.onDevice = onDevice
        isHidden = onDevice == nil
        backgroundColor = CGColor(gray: 0, alpha: 0.35)
        fill.backgroundColor = downloading ? tint : muted
        if !downloading { arriving = 0 }
        place()
        CATransaction.commit()
    }

    @MainActor
    func take(_ figures: [TransferFigure]) {
        arriving = figures.reduce(0) { $0 + ($1.progress ?? 0) }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        place()
        CATransaction.commit()
    }

    override func layoutSublayers() {
        super.layoutSublayers()
        place()
    }

    private func place() {
        var fraction = 0.0
        if let onDevice, onDevice.total > 0 {
            fraction = min((Double(onDevice.have) + arriving) / Double(onDevice.total), 1)
        }
        fill.frame = CGRect(x: 0, y: 0, width: bounds.width * fraction, height: bounds.height)
    }

    /// Where the bar sits on a sleeve `side` points across: along its foot,
    /// clear of the heart in the corner.
    static func frame(side: CGFloat, flipped: Bool) -> CGRect {
        CGRect(x: 8, y: flipped ? side - 8 - 3 : 8, width: max(side - 16 - 34, 0), height: 3)
    }
}

extension DownloadBarLayer: RecordGauge {}

extension EngineMirror {
    /// The tracks still to settle in the download store, by record.
    var arrivingByAlbum: [Int64: [Int64]] {
        Dictionary(
            grouping: transfers.filter { !$0.state.isSettled && $0.albumId != nil },
            by: { $0.albumId! }
        ).mapValues { $0.map(\.trackId) }
    }
}

#if !canImport(AppKit)
/// The bar on an iOS tile, for a record whose listing says how much is here.
struct DownloadedBar: UIViewRepresentable {
    let album: Album

    @Environment(EngineMirror.self) private var mirror
    @Environment(TransferMeter.self) private var meter

    func makeUIView(context: Context) -> BarView { BarView() }

    func updateUIView(_ view: BarView, context: Context) {
        let arriving = mirror.arrivingByAlbum[album.id] ?? []
        view.show(album.onDevice, downloading: !arriving.isEmpty)
        meter.follow(view.bar, transfers: arriving)
    }

    final class BarView: UIView {
        let bar = DownloadBarLayer()
        private var onDevice: AlbumOnDevice?
        private var downloading = false

        override init(frame: CGRect) {
            super.init(frame: frame)
            isUserInteractionEnabled = false
            isAccessibilityElement = false
            layer.addSublayer(bar)
        }

        required init?(coder: NSCoder) { fatalError("not decoded") }

        func show(_ onDevice: AlbumOnDevice?, downloading: Bool) {
            self.onDevice = onDevice
            self.downloading = downloading
            restyle()
        }

        override func tintColorDidChange() {
            super.tintColorDidChange()
            restyle()
        }

        override func traitCollectionDidChange(_ previous: UITraitCollection?) {
            super.traitCollectionDidChange(previous)
            restyle()
        }

        private func restyle() {
            bar.show(
                onDevice, downloading: downloading,
                tint: tintColor.resolvedColor(with: traitCollection).cgColor,
                muted: UIColor.secondaryLabel.resolvedColor(with: traitCollection).cgColor
            )
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            bar.frame = DownloadBarLayer.frame(side: bounds.width, flipped: true)
            CATransaction.commit()
        }
    }
}
#endif
