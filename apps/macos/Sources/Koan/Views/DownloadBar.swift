#if canImport(AppKit)
import AppKit
#else
import SwiftUI
import UIKit
#endif
import KoanFFI
import QuartzCore

/// How much of a record is on this device, filling up the left edge of its
/// sleeve, and whether more of it is arriving. Partly here and still, it is a
/// muted bar. While any of its tracks download it takes the tint and rises
/// with their bytes, which `TransferMeter` hands it at the display's rate;
/// when the last one settles it is still again. A record wholly here shows
/// none: there is nothing left to measure.
final class DownloadBarLayer: CALayer {
    private let fill = CALayer()
    private var onDevice: AlbumOnDevice?
    /// The in-flight tracks' progress, summed: how many tracks' worth of
    /// bytes have arrived for tracks not yet counted as here.
    private var arriving: Double = 0

    override init() {
        super.init()
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
        isHidden = onDevice.map { $0.have >= $0.total && !downloading } ?? true
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
        // Flipped geometry on both platforms: the foot is at maxY.
        let height = bounds.height * fraction
        fill.frame = CGRect(x: 0, y: bounds.height - height, width: bounds.width, height: height)
    }

    /// Where the bar sits on a sleeve `side` points across: flush along its
    /// left edge, the whole height, inside a layer clipped to the sleeve's
    /// corners.
    static func frame(side: CGFloat) -> CGRect {
        CGRect(x: 0, y: 0, width: 3, height: side)
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
            layer.cornerRadius = KoanTheme.radius(6)
            layer.cornerCurve = .continuous
            layer.masksToBounds = true
            layer.addSublayer(bar)
            registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: BarView, _) in
                view.restyle()
            }
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
            bar.frame = DownloadBarLayer.frame(side: bounds.width)
            CATransaction.commit()
        }
    }
}
#endif
