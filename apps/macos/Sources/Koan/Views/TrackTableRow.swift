#if os(macOS)
import AppKit
import KoanFFI

/// A line of a Mac track list: a track, or the heading over a run of them.
struct TrackLine: Equatable {
    enum Kind: Equatable {
        case track(Track)
        /// A day in history. Not selectable; stays put while its run scrolls.
        case heading(String)
    }

    /// The row's identity in its list: the track, or a history entry, which
    /// can be the same track twice.
    let id: Int64
    let kind: Kind
    /// What stands in the lead column: the track's number, the time it played.
    var lead = ""
    /// Something the row should say about itself, as a small mark with this
    /// as its tooltip — a play scrobbled by another client.
    var note: String?
    /// Where playing this row starts the list it is in.
    var position = 0

    var track: Track? {
        if case .track(let track) = kind { track } else { nil }
    }

    var isHeading: Bool {
        if case .heading = kind { true } else { false }
    }
}

extension TrackTableRow {
    /// A lead column wide enough for the longest number in a list of `count`.
    static func leadWidth(for count: Int) -> CGFloat {
        max(22, CGFloat(String(count).count) * 7 + 8)
    }
}

/// The columns a track list draws beside the title.
struct TrackColumns: OptionSet {
    let rawValue: Int
    /// Where the file is, or its download.
    static let availability = TrackColumns(rawValue: 1)
    static let heart = TrackColumns(rawValue: 2)
    static let quality = TrackColumns(rawValue: 4)
    static let all: TrackColumns = [.availability, .heart, .quality]
}

/// One track in a Mac track list, as `TrackRow` draws it in SwiftUI: its
/// number, or the bars when it is playing, or a play mark under the pointer;
/// the sleeve when the list is gathered from many records; the title over the
/// artist and record, which link out; where the file is; the heart; the
/// format; the length.
final class TrackTableRow: NSTableCellView, TableRow {
    struct Context {
        let showsAlbum: Bool
        var columns = TrackColumns.all
        var leadWidth: CGFloat = 22
        /// Whether the page is picking, and what — the lead is a tick then.
        var picking = false
        var picked: Set<Playable.Key> = []
        let currentTrackId: Int64?
        let isPlaying: Bool
        /// Whether the bars follow the music — see `PlayingIndicator.live`.
        let barsLive: Bool
        let tint: NSColor
        let favourites: Set<Int64>
        let queued: [Int64: QueueItem]
        let meter: TransferMeter
        let art: CoverArtCache
        let levels: PlayingLevels
        let play: (TrackLine) -> Void
        let openArtist: (Int64) -> Void
        let openAlbum: (Int64) -> Void
        let toggleFavourite: (Int64) -> Void
    }

    static let identifier = NSUserInterfaceItemIdentifier("TrackTableRow")
    static var height: CGFloat { RowMetrics.text + 2 * RowMetrics.padding }
    static var artHeight: CGFloat { RowMetrics.art + 2 * RowMetrics.padding }
    static let headingHeight: CGFloat = 28

    private static let titleFont = NSFont.role(.body, system: NSFont.preferredFont(forTextStyle: .body))
    private static let captionFont = NSFont.role(.meta, system: NSFont.preferredFont(forTextStyle: .caption1))
    private static let numberFont = NSFont.role(.meta, system: NSFont.monospacedDigitSystemFont(ofSize: captionFont.pointSize, weight: .regular))
    private static let qualityFont = NSFont.role(.fine, system: NSFont.monospacedSystemFont(
        ofSize: NSFont.preferredFont(forTextStyle: .caption2).pointSize, weight: .regular
    ))
    private static let spacing: CGFloat = 12
    /// Wide enough for an hour or more ("1:02:34") and for the widest format
    /// ("VORBIS 44.1 kHz", the lossy form) in whichever face the rows are drawn
    /// in; never narrower than the columns were.
    private static let durationWidth = max(48, measure("0:00:00", numberFont))
    private static let qualityWidth = max(92, measure("VORBIS 44.1 kHz", qualityFont))

    private static func measure(_ text: String, _ font: NSFont) -> CGFloat {
        ceil((text as NSString).size(withAttributes: [.font: font]).width) + 2
    }

    private enum Part { case lead, artist, album, heart, elsewhere }

    private static let headingFont = NSFont.role(.fine, system: NSFont.systemFont(
        ofSize: NSFont.preferredFont(forTextStyle: .subheadline).pointSize, weight: .semibold
    ))

    private let number = NSTextField(labelWithString: "")
    private let mark = CALayer()
    private var bars: PlayingBarsView?
    private let sleeve = CALayer()
    /// What a sleeve shows until, or instead of, its art: the ensō on a grey
    /// ground, as `AlbumArtwork` draws it, or a note for a track on no record.
    private let placeholder = CAShapeLayer()
    private let noRecord = CALayer()
    private let title = NSTextField(labelWithString: "")
    private let artist = NSTextField(labelWithString: "")
    private let dot = NSTextField(labelWithString: "·")
    private let album = NSTextField(labelWithString: "")
    private let availability = AvailabilityMark()
    private let heart = CALayer()
    private let quality = NSTextField(labelWithString: "")
    private let duration = NSTextField(labelWithString: "")
    private let heading = NSTextField(labelWithString: "")
    private let note = CALayer()
    private var noteImage: CGImage? { didSet { note.contents = noteImage } }

    private var item: TrackLine?
    private var context: Context?
    private var hovered: Part?
    private var sleeveLoad: Task<Void, Never>?
    private var markImage: CGImage? { didSet { mark.contents = markImage } }
    private var heartImage: CGImage? { didSet { heart.contents = heartImage } }

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        for layer in [mark, sleeve, availability, heart, note] { self.layer?.addSublayer(layer) }
        sleeve.cornerRadius = KoanTheme.radius(3)
        sleeve.masksToBounds = true
        sleeve.contentsGravity = .resizeAspectFill
        placeholder.fillColor = nil
        placeholder.lineCap = .round
        placeholder.opacity = 0.5
        sleeve.addSublayer(placeholder)
        noRecord.contentsGravity = .center
        sleeve.addSublayer(noRecord)
        for label in [number, title, artist, dot, album, quality, duration, heading] {
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            addSubview(label)
        }
        number.font = Self.numberFont
        number.alignment = .right
        title.font = Self.titleFont
        artist.font = Self.captionFont
        dot.font = Self.captionFont
        album.font = Self.captionFont
        quality.font = Self.qualityFont
        quality.alignment = .right
        duration.font = Self.numberFont
        duration.alignment = .right
        heading.font = Self.headingFont
        textField = title
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override var isFlipped: Bool { true }

    func show(_ item: TrackLine, in context: Context) {
        let fresh = self.item?.id != item.id || self.item?.kind != item.kind
        self.item = item
        self.context = context
        let isHeading = item.isHeading
        for view in [number, title, artist, dot, album, quality, duration] as [NSView] { view.isHidden = isHeading }
        for layer in [mark, sleeve, availability, heart, note] { layer.isHidden = isHeading }
        heading.isHidden = !isHeading
        if case .heading(let text) = item.kind {
            heading.stringValue = KoanTheme.label(text)
            context.meter.follow(availability, transfer: nil)
            needsLayout = true
            return
        }
        guard let track = item.track else { return }
        if fresh {
            hovered = nil
            number.stringValue = item.lead
            title.stringValue = Format.title(track.title)
            artist.stringValue = track.artistName
            artist.toolTip = track.artistId == nil ? nil : "Go to \(track.artistName)"
            album.stringValue = track.albumTitle
            album.toolTip = track.albumId == nil ? nil : "Go to \(track.albumTitle)"
            quality.stringValue = context.columns.contains(.quality) ? Format.quality(track) ?? "" : ""
            duration.stringValue = Format.duration(track.durationMs)
            showSleeve(track.albumId, art: context.art)
            setAccessibilityCustomActions([
                NSAccessibilityCustomAction(name: "Play") { [weak self] in self?.play() ?? false },
            ])
        }
        sleeve.isHidden = !context.showsAlbum
        dot.isHidden = !context.showsAlbum
        album.isHidden = !context.showsAlbum
        restyle()
    }

    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { restyle() }
    }

    /// Made before it is in the window, a row draws its symbols in whatever
    /// appearance it has then; drawn again once it has the window's.
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        restyle()
        paintSleeve()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        restyle()
        paintSleeve()
    }

    /// Everything that follows hover, the selection, what is playing, the
    /// favourites, downloads and the room's colour.
    private func restyle() {
        guard let item, let context, let track = item.track else {
            heading.textColor = .koanSecondaryLabel
            return
        }
        let selected = backgroundStyle == .emphasized
        let appearance = effectiveAppearance
        let current = context.currentTrackId == track.id
        let onAccent: NSColor = .alternateSelectedControlTextColor

        // The number, the bars, the play mark or a tick: one slot, so the
        // column does not twitch.
        let ticked = context.picked.contains(Playable.track(track).key)
        let showsMark = hovered != nil || context.picking
        number.isHidden = showsMark || current
        number.textColor = selected ? onAccent : .koanTertiaryLabel
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        mark.isHidden = !showsMark
        if context.picking {
            markImage = ticked
                ? Symbol.image("checkmark.circle.fill", size: 13, colours: [.white, context.tint], appearance: appearance)
                : Symbol.image("circle", size: 13, colours: [.koanTertiaryLabel], appearance: appearance)
        } else if showsMark {
            markImage = Symbol.image("play.circle.fill", size: 15, colours: [selected ? .white : context.tint], appearance: appearance)
        }
        CATransaction.commit()
        showBars(current && !showsMark, live: context.barsLive && context.isPlaying, context: context, selected: selected)

        title.textColor = current && !selected ? context.tint : (selected ? onAccent : .koanLabel)
        let artistLinked = hovered == .artist && track.artistId != nil
        let albumLinked = hovered == .album && track.albumId != nil
        style(artist, linked: artistLinked, selected: selected)
        style(album, linked: albumLinked, selected: selected)
        dot.textColor = selected ? onAccent : .koanTertiaryLabel
        quality.textColor = selected ? onAccent : .koanTertiaryLabel
        duration.textColor = selected ? onAccent : .koanSecondaryLabel

        availability.isHidden = !context.columns.contains(.availability)
        if context.columns.contains(.availability) {
            let queued = context.queued[track.id]
            let state = AvailabilityMark.state(
                onServer: track.onServer, onDisk: track.onDisk, queued: queued, meter: context.meter
            )
            availability.show(state, tint: context.tint, selected: selected, appearance: appearance)
            toolTip = AvailabilityMark.help(state, failure: queued?.failureReason)
            context.meter.follow(availability, transfer: state.isTransferring ? SourceBadges.transfer(of: queued) : nil)
        } else {
            context.meter.follow(availability, transfer: nil)
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        note.isHidden = item.note == nil
        if item.note != nil {
            noteImage = Symbol.image("antenna.radiowaves.left.and.right", size: 10, colours: [selected ? .white : .koanTertiaryLabel], appearance: appearance)
        }
        CATransaction.commit()
        toolTip = item.note ?? toolTip

        let favourite = context.favourites.contains(track.id)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        heart.isHidden = !context.columns.contains(.heart) || !(favourite || hovered != nil)
        heartImage = Symbol.image(
            favourite ? "heart.fill" : "heart", size: 12,
            colours: [favourite ? NSColor.koanBad(.systemRed) : (selected ? .white : .koanTertiaryLabel)], appearance: appearance
        )
        CATransaction.commit()
        needsLayout = true
    }

    private func style(_ label: NSTextField, linked: Bool, selected: Bool) {
        var attributes: [NSAttributedString.Key: Any] = [
            .font: Self.captionFont,
            .foregroundColor: selected
                ? NSColor.alternateSelectedControlTextColor
                : (linked ? NSColor.koanLabel : NSColor.koanSecondaryLabel),
        ]
        if linked { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
        label.attributedStringValue = NSAttributedString(string: label.stringValue, attributes: attributes)
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

    private func showSleeve(_ albumId: Int64?, art: CoverArtCache) {
        sleeveLoad?.cancel()
        guard let albumId else {
            setSleeve(nil, onRecord: false)
            return
        }
        if let held = art.cached(.album(albumId), size: .thumb) {
            setSleeve(held.bitmap, onRecord: true)
            return
        }
        setSleeve(nil, onRecord: true)
        sleeveLoad = Task { [weak self] in
            let image = await art.image(for: .album(albumId), size: .thumb)
            guard !Task.isCancelled, let self, self.item?.track?.albumId == albumId else { return }
            self.setSleeve(image?.bitmap, onRecord: true)
        }
    }

    private func setSleeve(_ image: CGImage?, onRecord: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        sleeve.contents = image
        placeholder.isHidden = image != nil || !onRecord
        noRecord.isHidden = onRecord
        CATransaction.commit()
        paintSleeve()
    }

    private func paintSleeve() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            sleeve.backgroundColor = sleeve.contents == nil ? NSColor.koanQuaternaryLabel.cgColor : nil
            placeholder.strokeColor = NSColor.koanTertiaryLabel.cgColor
            noRecord.contents = Symbol.image("music.note", size: 10, colours: [.koanTertiaryLabel], appearance: effectiveAppearance)
            CATransaction.commit()
        }
    }

    // MARK: Layout

    override func layout() {
        super.layout()
        guard let context else { return }
        let height = bounds.height
        let lineHeight = { (font: NSFont) in ceil(font.ascender - font.descender + font.leading) }
        if item?.isHeading == true {
            let headingHeight = lineHeight(Self.headingFont)
            heading.frame = CGRect(x: 0, y: height - headingHeight - 4, width: bounds.width, height: headingHeight)
            return
        }
        var x: CGFloat = 0

        let lead = CGRect(x: x, y: 0, width: context.leadWidth, height: height)
        let numberHeight = lineHeight(Self.numberFont)
        number.frame = CGRect(x: lead.minX, y: (height - numberHeight) / 2, width: lead.width, height: numberHeight)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        mark.frame = Symbol.frame(of: markImage, centredIn: lead.offsetBy(dx: 4, dy: 0))
        if let bars {
            let size = bars.intrinsicContentSize
            bars.frame = CGRect(x: lead.maxX - size.width, y: (height - size.height) / 2, width: size.width, height: size.height)
        }
        x = lead.maxX + Self.spacing

        if context.showsAlbum {
            let side = RowMetrics.sleeve
            sleeve.frame = CGRect(x: x, y: (height - side) / 2, width: side, height: side)
            let inset = side * 0.26
            placeholder.frame = sleeve.bounds
            placeholder.lineWidth = side * 0.045
            placeholder.path = EnsoShape().path(in: CGRect(x: inset, y: inset, width: side - 2 * inset, height: side - 2 * inset)).cgPath
            noRecord.frame = sleeve.bounds
            x += side + Self.spacing
        }

        var right = bounds.width
        let durationWidth = Self.durationWidth
        let durationHeight = lineHeight(Self.numberFont)
        duration.frame = CGRect(x: right - durationWidth, y: (height - durationHeight) / 2, width: durationWidth, height: durationHeight)
        right -= durationWidth + Self.spacing
        if !quality.stringValue.isEmpty {
            let qualityHeight = lineHeight(Self.qualityFont)
            quality.frame = CGRect(
                x: right - Self.qualityWidth, y: (height - qualityHeight) / 2,
                width: Self.qualityWidth, height: qualityHeight
            )
            right -= Self.qualityWidth + Self.spacing
        }
        if context.columns.contains(.heart) {
            heart.frame = Symbol.frame(of: heartImage, centredIn: CGRect(x: right - 16, y: 0, width: 16, height: height))
            right -= 16 + Self.spacing
        }
        if item?.note != nil {
            note.frame = Symbol.frame(of: noteImage, centredIn: CGRect(x: right - 16, y: 0, width: 16, height: height))
            right -= 16 + Self.spacing
        }
        if context.columns.contains(.availability) {
            let slot = CGRect(x: right - 30, y: (height - 16) / 2, width: 30, height: 16)
            availability.frame = slot
            right = slot.minX - 8
        }
        CATransaction.commit()

        let titleHeight = lineHeight(Self.titleFont)
        let captionHeight = lineHeight(Self.captionFont)
        let top = (height - titleHeight - 1 - captionHeight) / 2
        let width = max(right - x, 0)
        title.frame = CGRect(x: x, y: top, width: width, height: titleHeight)
        let captionY = top + titleHeight + 1
        let measure = { (text: String) in ceil((text as NSString).size(withAttributes: [.font: Self.captionFont]).width) + 5 }
        let artistWidth = min(measure(artist.stringValue), width)
        artist.frame = CGRect(x: x, y: captionY, width: artistWidth, height: captionHeight)
        if context.showsAlbum {
            dot.frame = CGRect(x: artist.frame.maxX, y: captionY, width: 10, height: captionHeight)
            let albumX = dot.frame.maxX + 2
            album.frame = CGRect(x: albumX, y: captionY, width: min(measure(album.stringValue), max(x + width - albumX, 0)), height: captionHeight)
        }
    }

    // MARK: Pointer

    private func part(at point: NSPoint) -> Part {
        if item?.isHeading != false { return .elsewhere }
        if point.x < (context?.leadWidth ?? 22) + 4 { return .lead }
        if artist.frame.contains(point) { return .artist }
        if context?.showsAlbum == true, album.frame.contains(point) { return .album }
        if context?.columns.contains(.heart) == true, heart.frame.insetBy(dx: -3, dy: -3).contains(point) { return .heart }
        return .elsewhere
    }

    func hover(at point: NSPoint?) -> Bool {
        let now = point.map(part(at:))
        if now != hovered {
            hovered = now
            restyle()
        }
        guard let track = item?.track else { return false }
        return (now == .artist && track.artistId != nil) || (now == .album && track.albumId != nil)
    }

    func hit(at point: NSPoint) -> RowHit {
        guard let item, let context, let track = item.track else { return .plain }
        if context.picking { return .plain }
        switch part(at: point) {
        case .lead: return .button { [weak self] in _ = self?.play() }
        case .heart: return .button { context.toggleFavourite(track.id) }
        case .artist:
            guard let id = track.artistId else { return .plain }
            return .link { context.openArtist(id) }
        case .album:
            guard let id = track.albumId else { return .plain }
            return .link { context.openAlbum(id) }
        case .elsewhere: return .plain
        }
    }

    @discardableResult
    private func play() -> Bool {
        guard let item, let context else { return false }
        context.play(item)
        return true
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        sleeveLoad?.cancel()
        hovered = nil
        if let bars, let context {
            context.levels.detach(bars)
            bars.removeFromSuperview()
            self.bars = nil
        }
    }
}
#endif
