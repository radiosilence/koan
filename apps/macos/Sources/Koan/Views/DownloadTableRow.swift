#if os(macOS)
import AppKit
import KoanFFI

/// One transfer in the Mac's downloads list, as `DownloadRow` draws it in
/// SwiftUI: the sleeve; the title and how far along; a bar; who it is by and
/// how it is going, with a link to the record under the pointer.
final class DownloadTableRow: NSTableCellView, TableRow, TransferGauge {
    struct Context {
        let meter: TransferMeter
        let art: CoverArtCache
        let showInLibrary: (Transfer) -> Void
    }

    static let identifier = NSUserInterfaceItemIdentifier("DownloadTableRow")
    static let height: CGFloat = 60

    private static let titleFont = NSFont.role(.body, system: NSFont.preferredFont(forTextStyle: .body))
    private static let captionFont = NSFont.role(.meta, system: NSFont.preferredFont(forTextStyle: .caption1))
    private static let figureFont = NSFont.role(.meta, system: NSFont.monospacedDigitSystemFont(ofSize: captionFont.pointSize, weight: .regular))

    private let sleeve = CALayer()
    private let track = CALayer()
    private let filled = CALayer()
    private let title = NSTextField(labelWithString: "")
    private let figure = NSTextField(labelWithString: "")
    private let subtitle = NSTextField(labelWithString: "")
    private let link = NSTextField(labelWithString: "Show in Library")

    private var transfer: Transfer?
    private var context: Context?
    private var fraction: Double = 0
    private var hovered = false
    private var sleeveLoad: Task<Void, Never>?

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        for layer in [sleeve, track, filled] { self.layer?.addSublayer(layer) }
        sleeve.cornerRadius = KoanTheme.radius(3)
        sleeve.masksToBounds = true
        sleeve.contentsGravity = .resizeAspectFill
        track.cornerRadius = KoanTheme.radius(2)
        filled.cornerRadius = KoanTheme.radius(2)
        for label in [title, figure, subtitle, link] {
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            addSubview(label)
        }
        title.font = Self.titleFont
        figure.font = Self.figureFont
        figure.alignment = .right
        subtitle.font = Self.captionFont
        link.font = Self.captionFont
        link.alignment = .right
        textField = title
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override var isFlipped: Bool { true }

    func show(_ transfer: Transfer, in context: Context) {
        if self.transfer?.trackId != transfer.trackId {
            hovered = false
            showSleeve(transfer.trackId, art: context.art)
        }
        self.transfer = transfer
        self.context = context
        title.stringValue = transfer.title

        // The numbers only while it moves; a settled row does not read them.
        let running = transfer.state == .running
        let figures = running ? context.meter.figure(for: transfer.trackId) : nil
        switch transfer.state {
        case .done: figure.stringValue = "Done"
        case .failed: figure.stringValue = "Failed"
        case .queued: figure.stringValue = "Queued"
        case .running: figure.stringValue = Self.percent(figures)
        }
        // Whole once it has landed; empty, not full, when no length was given.
        fraction = transfer.state == .done ? 1 : figures?.progress ?? 0
        subtitle.stringValue = Self.subtitle(transfer, figures: figures)
        restyle()
        context.meter.follow(self, transfer: running ? transfer.trackId : nil)
    }

    /// A frame's figures, between the table's own redraws: the bar by its
    /// width, the text only when it reads differently.
    func take(_ figures: TransferFigure) {
        guard let transfer, transfer.state == .running else { return }
        let percent = Self.percent(figures)
        if figure.stringValue != percent {
            figure.stringValue = percent
            needsLayout = true
        }
        let status = Self.subtitle(transfer, figures: figures)
        if subtitle.stringValue != status { subtitle.stringValue = status }
        fraction = figures.progress ?? 0
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        filled.frame.size.width = track.frame.width * fraction
        CATransaction.commit()
    }

    private static func percent(_ figures: TransferFigure?) -> String {
        figures?.progress.map { "\(Int($0 * 100))%" } ?? ""
    }

    private static func subtitle(_ transfer: Transfer, figures: TransferFigure?) -> String {
        switch transfer.state {
        case .failed: return transfer.failureReason ?? "Couldn't be fetched"
        case .queued: return transfer.artist.isEmpty ? "Waiting" : "\(transfer.artist) — waiting"
        case .done: return transfer.artist.isEmpty ? "Downloaded" : "\(transfer.artist) — downloaded"
        case .running:
            // A rate of nothing is a stall, and saying so is the point of the row.
            var parts: [String] = []
            if !transfer.artist.isEmpty { parts.append(transfer.artist) }
            let rate = figures?.bytesPerSecond ?? 0
            parts.append(rate > 0 ? "\(Format.bytes(Int64(rate)))/s" : "stalled")
            let written = figures?.bytesWritten ?? 0
            let total = figures?.totalBytes ?? 0
            if total > 0 {
                parts.append("\(Format.bytes(Int64(written))) of \(Format.bytes(Int64(total)))")
            } else if written > 0 {
                parts.append(Format.bytes(Int64(written)))
            }
            return parts.joined(separator: " · ")
        }
    }

    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { restyle() }
    }

    /// Made before it is in the window, a row draws its symbols in whatever
    /// appearance it has then; drawn again once it has the window's.
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        restyle()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        restyle()
    }

    private func restyle() {
        guard let transfer else { return }
        let selected = backgroundStyle == .emphasized
        let onAccent: NSColor = .alternateSelectedControlTextColor
        title.textColor = selected ? onAccent : .koanLabel
        figure.textColor = selected ? onAccent : .koanSecondaryLabel
        subtitle.textColor = selected ? onAccent : (transfer.state == .failed ? NSColor.koanBad(.systemOrange) : .koanSecondaryLabel)
        link.isHidden = !hovered
        var attributes: [NSAttributedString.Key: Any] = [
            .font: Self.captionFont, .foregroundColor: selected ? onAccent : NSColor.linkColor,
        ]
        attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue
        link.attributedStringValue = NSAttributedString(string: "Show in Library", attributes: attributes)
        effectiveAppearance.performAsCurrentDrawingAppearance {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            // The quiet end is what has not arrived, the same way round as the
            // seek bar, so a finished transfer reads as full.
            track.backgroundColor = NSColor.koanQuaternaryLabel.cgColor
            filled.backgroundColor = (selected ? NSColor.white : NSColor.koanLabel).cgColor
            CATransaction.commit()
        }
        needsLayout = true
    }

    private func showSleeve(_ trackId: Int64, art: CoverArtCache) {
        sleeveLoad?.cancel()
        if let held = art.cached(.track(trackId), size: .thumb) {
            sleeve.contents = held.bitmap
            return
        }
        sleeve.contents = nil
        sleeveLoad = Task { [weak self] in
            let image = await art.image(for: .track(trackId), size: .thumb)
            guard !Task.isCancelled, let self, self.transfer?.trackId == trackId else { return }
            self.sleeve.contents = image?.bitmap
        }
    }

    override func layout() {
        super.layout()
        guard let transfer else { return }
        let lineHeight = { (font: NSFont) in ceil(font.ascender - font.descender + font.leading) }
        let side = RowMetrics.sleeve
        // A finished transfer has nothing left to measure, so its bar goes and
        // the title and status close up around the middle.
        let showsBar = transfer.state != .done
        let x = side + 10
        let width = bounds.width - x
        let titleHeight = lineHeight(Self.titleFont)
        let captionHeight = lineHeight(Self.captionFont)
        let barBlock: CGFloat = showsBar ? 5 + 4 + 5 : 3
        let top = (bounds.height - (titleHeight + barBlock + captionHeight)) / 2

        let figureWidth = ceil((figure.stringValue as NSString).size(withAttributes: [.font: Self.figureFont]).width) + 5
        title.frame = CGRect(x: x, y: top, width: max(width - figureWidth - 8, 0), height: titleHeight)
        figure.frame = CGRect(x: bounds.width - figureWidth, y: top + (titleHeight - captionHeight) / 2, width: figureWidth, height: captionHeight)

        let barY = top + titleHeight + 5
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        sleeve.frame = CGRect(x: 0, y: (bounds.height - side) / 2, width: side, height: side)
        track.isHidden = !showsBar
        filled.isHidden = !showsBar
        track.frame = CGRect(x: x, y: barY, width: width, height: 4)
        filled.frame = CGRect(x: x, y: barY, width: width * fraction, height: 4)
        CATransaction.commit()

        let captionY = top + titleHeight + barBlock
        let linkWidth = ceil((link.stringValue as NSString).size(withAttributes: [.font: Self.captionFont]).width) + 5
        link.frame = CGRect(x: bounds.width - linkWidth, y: captionY, width: linkWidth, height: captionHeight)
        subtitle.frame = CGRect(x: x, y: captionY, width: max(width - (hovered ? linkWidth + 8 : 0), 0), height: captionHeight)
    }

    // MARK: Pointer

    func hover(at point: NSPoint?) -> Bool {
        let now = point != nil
        if now != hovered {
            hovered = now
            restyle()
        }
        return point.map { link.frame.contains($0) } ?? false
    }

    func hit(at point: NSPoint) -> RowHit {
        guard let transfer, let context, hovered, link.frame.contains(point) else { return .plain }
        return .button { context.showInLibrary(transfer) }
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        sleeveLoad?.cancel()
        hovered = false
    }
}
#endif
