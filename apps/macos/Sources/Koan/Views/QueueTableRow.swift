#if os(macOS)
import AppKit
import KoanFFI

/// A line of the Mac's queue or a playlist: a record's heading over its run,
/// or one track.
struct QueueLine: Equatable {
    enum Kind: Equatable {
        case heading(QueueHeading)
        case track(QueueRowContent, isCurrent: Bool, showArtist: Bool, artwork: Bool)
    }

    /// The row's identity: a queue item, a playlist entry, or a heading.
    let id: String
    let kind: Kind

    var isTrack: Bool {
        if case .track = kind { true } else { false }
    }
}

/// A record heading a run of its tracks, as `QueueAlbumHeader` and
/// `PlaylistAlbumHeader` draw it.
struct QueueHeading: Equatable {
    let title: String
    /// The record's artist, under its title.
    let artist: String?
    /// "2007 · 11 tracks · 59:10 · FLAC", where the list says it.
    let detail: String?
    let sleeve: AlbumArtwork.Source?
    let sleeveSize: CGFloat
}

/// One line of the queue or a playlist, as `QueueRow` draws a track in
/// SwiftUI: what the track is doing, its place or its sleeve, its title over
/// its artist, where its file is, the heart, the codec, the length. Played
/// tracks dim in their colours, not their alpha.
final class QueueTableRow: NSTableCellView, TableRow {
    struct Context {
        let isPlaying: Bool
        /// Whether the bars follow the music — see `PlayingIndicator.live`.
        let barsLive: Bool
        let tint: NSColor
        let favourites: Set<Int64>
        let progress: (String) -> Double?
        let art: CoverArtCache
        let levels: PlayingLevels
        let toggleFavourite: (Int64) -> Void
    }

    static let identifier = NSUserInterfaceItemIdentifier("QueueTableRow")
    static let height = RowMetrics.text + 2 * RowMetrics.padding
    static let artHeight = RowMetrics.art + 2 * RowMetrics.artPadding + 2 * RowMetrics.padding

    static func height(of line: QueueLine) -> CGFloat {
        switch line.kind {
        case .heading(let heading): heading.sleeveSize + 2 * (heading.detail == nil ? 5 : 6) + 2 * RowMetrics.padding
        case .track(_, _, _, let artwork): artwork ? artHeight : height
        }
    }

    private static let titleFont = NSFont.preferredFont(forTextStyle: .body)
    private static let captionFont = NSFont.preferredFont(forTextStyle: .caption1)
    private static let numberFont = NSFont.monospacedDigitSystemFont(ofSize: captionFont.pointSize, weight: .regular)
    private static let codecFont = NSFont.monospacedSystemFont(
        ofSize: NSFont.preferredFont(forTextStyle: .caption2).pointSize, weight: .regular
    )
    private static let headingFont = NSFont.systemFont(ofSize: 14, weight: .semibold)
    private static let detailFont = NSFont.monospacedDigitSystemFont(
        ofSize: NSFont.preferredFont(forTextStyle: .caption2).pointSize, weight: .regular
    )

    private let status = CALayer()
    private var bars: PlayingBarsView?
    private let shade = CALayer()
    private let sleeve = CALayer()
    private let number = NSTextField(labelWithString: "")
    private let title = NSTextField(labelWithString: "")
    private let artist = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")
    private let availability = AvailabilityMark()
    private let heart = CALayer()
    private let codec = NSTextField(labelWithString: "")
    private let duration = NSTextField(labelWithString: "")

    private var line: QueueLine?
    private var context: Context?
    private var hovered = false
    private var sleeveLoad: Task<Void, Never>?
    private var shownSleeve: AlbumArtwork.Source?
    private var statusImage: CGImage? { didSet { status.contents = statusImage } }
    private var heartImage: CGImage? { didSet { heart.contents = heartImage } }

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        shade.shadowOpacity = 0.28
        shade.shadowRadius = 4
        shade.shadowOffset = CGSize(width: 0, height: 2)
        sleeve.masksToBounds = true
        sleeve.contentsGravity = .resizeAspectFill
        for layer in [status, shade, sleeve, availability, heart] { self.layer?.addSublayer(layer) }
        for label in [number, title, artist, detail, codec, duration] {
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            addSubview(label)
        }
        number.font = Self.numberFont
        number.alignment = .right
        artist.font = Self.captionFont
        codec.font = Self.codecFont
        duration.font = Self.numberFont
        duration.alignment = .right
        detail.font = Self.detailFont
        textField = title
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override var isFlipped: Bool { true }

    func show(_ line: QueueLine, in context: Context) {
        let fresh = self.line?.id != line.id
        self.line = line
        self.context = context
        if fresh { hovered = false }

        switch line.kind {
        case .heading(let heading):
            title.font = Self.headingFont
            title.stringValue = Format.title(heading.title)
            artist.stringValue = heading.artist ?? ""
            detail.stringValue = heading.detail ?? ""
            showSleeve(heading.sleeve, art: context.art, corner: 5)
        case .track(let content, _, let showArtist, let artwork):
            title.font = Self.titleFont
            title.stringValue = Format.title(content.title)
            artist.stringValue = !showArtist || content.artist.isEmpty
                ? ""
                : (artwork && !content.album.isEmpty ? "\(content.artist) — \(content.album)" : content.artist)
            number.stringValue = content.number.map(String.init) ?? ""
            codec.stringValue = content.codec?.uppercased() ?? ""
            duration.stringValue = content.durationMs.map { Format.duration($0) } ?? ""
            showSleeve(artwork ? content.sleeve : nil, art: context.art, corner: 3)
        }
        restyle()
    }

    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { restyle() }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        restyle()
    }

    private func restyle() {
        guard let line, let context else { return }
        let selected = backgroundStyle == .emphasized
        let appearance = effectiveAppearance
        let onAccent: NSColor = .alternateSelectedControlTextColor
        let heading = !line.isTrack
        for view in [number, codec, duration] as [NSView] { view.isHidden = heading }
        detail.isHidden = !heading || detail.stringValue.isEmpty
        artist.isHidden = artist.stringValue.isEmpty

        if case .heading = line.kind {
            showBars(false, live: false, context: context, selected: selected)
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            status.isHidden = true
            availability.isHidden = true
            heart.isHidden = true
            sleeve.opacity = 1
            CATransaction.commit()
            title.textColor = selected ? onAccent : .labelColor
            artist.textColor = selected ? onAccent : .secondaryLabelColor
            detail.textColor = selected ? onAccent : .tertiaryLabelColor
            needsLayout = true
            return
        }
        guard case .track(let content, let isCurrent, _, let artwork) = line.kind else { return }
        let played = content.status == .played

        showBars(content.status == .playing, live: context.barsLive && context.isPlaying, context: context, selected: selected)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        status.isHidden = false
        switch content.status {
        case .priorityPending:
            statusImage = Symbol.image("arrow.down.circle", size: 10, colours: [selected ? .white : context.tint], appearance: appearance)
        case .failed:
            statusImage = Symbol.image("exclamationmark.triangle.fill", size: 10, colours: [.systemOrange], appearance: appearance)
        case .played:
            statusImage = Symbol.image("checkmark", size: 10, colours: [selected ? .white : .tertiaryLabelColor], appearance: appearance)
        case .queued:
            statusImage = Symbol.image("circle.dotted", size: 10, colours: [selected ? .white : .quaternaryLabelColor], appearance: appearance)
        default:
            statusImage = nil
        }
        // The one thing a colour cannot dim.
        sleeve.opacity = artwork && played ? 0.5 : 1
        availability.isHidden = false
        let state: AvailabilityMark.State = content.transferring.map { .transferring(context.progress($0)) }
            ?? (content.onServer || content.onDisk ? .stored(onServer: content.onServer, onDisk: content.onDisk) : .nothing)
        availability.show(state, tint: context.tint, selected: selected, appearance: appearance)
        let favourite = content.trackId.map(context.favourites.contains) ?? false
        heart.isHidden = content.trackId == nil || !(favourite || hovered)
        heartImage = Symbol.image(
            favourite ? "heart.fill" : "heart", size: 10,
            colours: [favourite ? .systemRed : (selected ? .white : .tertiaryLabelColor)], appearance: appearance
        )
        CATransaction.commit()

        title.textColor = isCurrent && !selected ? context.tint : (selected ? onAccent : (played ? .secondaryLabelColor : .labelColor))
        artist.textColor = selected ? onAccent : (played ? .tertiaryLabelColor : .secondaryLabelColor)
        number.textColor = selected ? onAccent : (played ? .quaternaryLabelColor : .tertiaryLabelColor)
        codec.textColor = selected ? onAccent : (played ? .quaternaryLabelColor : .tertiaryLabelColor)
        duration.textColor = selected ? onAccent : (played ? .tertiaryLabelColor : .secondaryLabelColor)
        toolTip = content.status == .failed ? content.failureReason ?? "Couldn't be fetched"
            : (content.status == .priorityPending ? "Queued for download" : nil)
        needsLayout = true
    }

    /// The playing row's bars, made for that row only — see `PlayingIndicator`.
    private func showBars(_ shown: Bool, live: Bool, context: Context, selected: Bool) {
        if shown, bars == nil {
            let made = PlayingBarsView()
            addSubview(made)
            bars = made
            needsLayout = true
        } else if !shown, let bars {
            context.levels.detach(bars)
            bars.removeFromSuperview()
            self.bars = nil
        }
        guard let bars else { return }
        bars.tint = selected ? .white : context.tint
        bars.source = context.levels
        if live {
            context.levels.attach(bars)
        } else {
            context.levels.detach(bars)
            bars.rest()
        }
    }

    private func showSleeve(_ source: AlbumArtwork.Source?, art: CoverArtCache, corner: CGFloat) {
        sleeve.cornerRadius = corner
        guard source != shownSleeve || sleeve.contents == nil else { return }
        shownSleeve = source
        sleeveLoad?.cancel()
        sleeve.isHidden = source == nil
        shade.isHidden = source == nil || line?.isTrack == true
        guard let source else { return }
        effectiveAppearance.performAsCurrentDrawingAppearance {
            sleeve.backgroundColor = NSColor.quaternaryLabelColor.cgColor
        }
        if let held = art.cached(source, size: .thumb) {
            sleeve.contents = held.bitmap
            return
        }
        sleeve.contents = nil
        sleeveLoad = Task { [weak self] in
            let image = await art.image(for: source, size: .thumb)
            guard !Task.isCancelled, let self, self.shownSleeve == source else { return }
            self.sleeve.contents = image?.bitmap
        }
    }

    // MARK: Layout

    override func layout() {
        super.layout()
        guard let line else { return }
        let height = bounds.height
        let lineHeight = { (font: NSFont) in ceil(font.ascender - font.descender + font.leading) }

        if case .heading(let heading) = line.kind {
            let side = heading.sleeveSize
            var x: CGFloat = 0
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            if heading.sleeve != nil {
                sleeve.frame = CGRect(x: 0, y: (height - side) / 2, width: side, height: side)
                shade.frame = sleeve.frame
                shade.shadowPath = CGPath(roundedRect: CGRect(origin: .zero, size: sleeve.frame.size), cornerWidth: 5, cornerHeight: 5, transform: nil)
                x = side + 12
            }
            CATransaction.commit()
            let titleHeight = lineHeight(Self.headingFont)
            let captionHeight = lineHeight(Self.captionFont)
            let detailHeight = detail.isHidden ? 0 : lineHeight(Self.detailFont) + 2
            let block = titleHeight + (artist.isHidden ? 0 : 2 + captionHeight) + detailHeight
            var y = (height - block) / 2
            let width = max(bounds.width - x, 0)
            title.frame = CGRect(x: x, y: y, width: width, height: titleHeight)
            y += titleHeight + 2
            if !artist.isHidden {
                artist.frame = CGRect(x: x, y: y, width: width, height: captionHeight)
                y += captionHeight + 2
            }
            if !detail.isHidden {
                detail.frame = CGRect(x: x, y: y, width: width, height: lineHeight(Self.detailFont))
            }
            return
        }

        guard case .track(_, _, _, let artwork) = line.kind else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        // The status and the place sit together: what this track is doing and
        // where it is are one thing.
        let statusSlot = CGRect(x: 0, y: 0, width: 16, height: height)
        status.frame = Symbol.frame(of: statusImage, centredIn: CGRect(x: statusSlot.maxX - 12, y: 0, width: 12, height: height))
        if let bars {
            let size = bars.intrinsicContentSize
            bars.frame = CGRect(x: statusSlot.maxX - size.width, y: (height - size.height) / 2, width: size.width, height: size.height)
        }
        var x = statusSlot.maxX + (artwork ? 10 : 6)
        if artwork {
            let side = RowMetrics.sleeve
            sleeve.frame = CGRect(x: x, y: (height - side) / 2, width: side, height: side)
            x += side + 10
        } else {
            let numberHeight = lineHeight(Self.numberFont)
            number.frame = CGRect(x: x, y: (height - numberHeight) / 2, width: 20, height: numberHeight)
            x += 20 + 10
        }

        var right = bounds.width
        let durationWidth: CGFloat = 44
        let numberHeight = lineHeight(Self.numberFont)
        duration.frame = CGRect(x: right - durationWidth, y: (height - numberHeight) / 2, width: durationWidth, height: numberHeight)
        right -= durationWidth + 10
        if !codec.stringValue.isEmpty {
            let codecWidth = ceil((codec.stringValue as NSString).size(withAttributes: [.font: Self.codecFont]).width) + 5
            let codecHeight = lineHeight(Self.codecFont)
            codec.frame = CGRect(x: right - codecWidth, y: (height - codecHeight) / 2, width: codecWidth, height: codecHeight)
            right -= codecWidth + 10
        }
        heart.frame = Symbol.frame(of: heartImage, centredIn: CGRect(x: right - 16, y: 0, width: 16, height: height))
        right -= 16 + 10
        availability.frame = CGRect(x: right - 14, y: (height - 14) / 2, width: 14, height: 14)
        right -= 14 + 8
        CATransaction.commit()

        let titleHeight = lineHeight(Self.titleFont)
        let captionHeight = lineHeight(Self.captionFont)
        let block = titleHeight + (artist.isHidden ? 0 : 1 + captionHeight)
        let top = (height - block) / 2
        let width = max(right - x, 0)
        title.frame = CGRect(x: x, y: top, width: width, height: titleHeight)
        artist.frame = CGRect(x: x, y: top + titleHeight + 1, width: width, height: captionHeight)
    }

    // MARK: Pointer

    func hover(at point: NSPoint?) -> Bool {
        let now = point != nil
        if now != hovered {
            hovered = now
            restyle()
        }
        return false
    }

    func hit(at point: NSPoint) -> RowHit {
        guard let line, let context, case .track(let content, _, _, _) = line.kind,
              let trackId = content.trackId, !heart.isHidden,
              heart.frame.insetBy(dx: -3, dy: -3).contains(point)
        else { return .plain }
        return .button { context.toggleFavourite(trackId) }
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        sleeveLoad?.cancel()
        hovered = false
        if let bars, let context {
            context.levels.detach(bars)
            bars.removeFromSuperview()
            self.bars = nil
        }
    }
}
#endif
