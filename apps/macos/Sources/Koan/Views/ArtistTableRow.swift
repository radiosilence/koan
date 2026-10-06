#if os(macOS)
import AppKit
import KoanFFI

/// One artist in the Mac's artist list, as `ArtistRow` draws it in SwiftUI:
/// the mic, or a play mark under the pointer; the name, which opens the
/// artist; the heart; how many records and tracks.
final class ArtistTableRow: NSTableCellView, TableRow {
    struct Context {
        let favourites: Set<Int64>
        let tint: NSColor
        let play: (Artist) async -> Void
        let open: (Int64) -> Void
        let toggleFavourite: (Int64) -> Void
    }

    static let identifier = NSUserInterfaceItemIdentifier("ArtistTableRow")
    static let height = RowMetrics.line + 2 * RowMetrics.padding

    private static let nameFont = NSFont.role(.body, system: NSFont.preferredFont(forTextStyle: .body))
    private static let countFont = NSFont.role(.meta, system: NSFont.monospacedDigitSystemFont(
        ofSize: NSFont.preferredFont(forTextStyle: .caption1).pointSize, weight: .regular
    ))
    private static let countWidth: CGFloat = 78

    private enum Part { case mark, name, heart, elsewhere }

    private let mark = CALayer()
    private let heart = CALayer()
    private var markImage: CGImage? {
        didSet { mark.contents = markImage }
    }
    private var heartImage: CGImage? {
        didSet { heart.contents = heartImage }
    }
    private let name = NSTextField(labelWithString: "")
    private let albums = NSTextField(labelWithString: "")
    private let tracks = NSTextField(labelWithString: "")

    private var artist: Artist?
    private var context: Context?
    private var hovered: Part?
    private var playing = false

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.addSublayer(mark)
        layer?.addSublayer(heart)
        for label in [name, albums, tracks] {
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            addSubview(label)
        }
        name.font = Self.nameFont
        albums.font = Self.countFont
        albums.alignment = .right
        tracks.font = Self.countFont
        tracks.alignment = .right
        // What type-to-select and VoiceOver read for the row.
        textField = name
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override var isFlipped: Bool { true }

    func show(_ artist: Artist, in context: Context) {
        if self.artist?.id != artist.id {
            hovered = nil
            playing = false
        }
        self.artist = artist
        self.context = context
        name.stringValue = artist.name
        albums.stringValue = Format.count(artist.albumCount, "album")
        tracks.stringValue = Format.count(artist.trackCount, "track")
        setAccessibilityCustomActions([
            NSAccessibilityCustomAction(name: "Play") { [weak self] in self?.play() ?? false },
            NSAccessibilityCustomAction(name: "Open") { [weak self] in
                self?.context?.open(artist.id)
                return true
            },
        ])
        restyle()
        needsLayout = true
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

    /// Everything that follows hover, the selection, the favourites and the
    /// room's colour.
    private func restyle() {
        guard let artist, let context else { return }
        let selected = backgroundStyle == .emphasized
        let appearance = effectiveAppearance
        let favourite = context.favourites.contains(artist.id)

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if hovered != nil || playing {
            markImage = Symbol.image(
                "play.circle.fill", size: 15, colours: [selected ? .white : context.tint], appearance: appearance
            )
        } else {
            markImage = Symbol.image(
                "music.mic", size: 10, colours: [selected ? .white : .koanTertiaryLabel], appearance: appearance
            )
        }
        heart.isHidden = !(favourite || hovered != nil)
        heartImage = Symbol.image(
            favourite ? "heart.fill" : "heart", size: 10,
            colours: [favourite ? NSColor.koanBad(.systemRed) : (selected ? .white : .koanTertiaryLabel)], appearance: appearance
        )
        CATransaction.commit()

        let linked = hovered == .name
        var attributes: [NSAttributedString.Key: Any] = [
            .font: Self.nameFont,
            .foregroundColor: selected ? NSColor.alternateSelectedControlTextColor : NSColor.koanLabel,
        ]
        if linked { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
        name.attributedStringValue = NSAttributedString(string: artist.name, attributes: attributes)
        albums.textColor = selected ? .alternateSelectedControlTextColor : .koanSecondaryLabel
        tracks.textColor = selected ? .alternateSelectedControlTextColor : .koanTertiaryLabel
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let height = bounds.height
        let markSize = CGSize(width: 18, height: 18)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        mark.frame = Symbol.frame(of: markImage, centredIn: CGRect(origin: CGPoint(x: 0, y: (height - markSize.height) / 2), size: markSize))
        CATransaction.commit()

        let lineHeight = ceil(Self.nameFont.ascender - Self.nameFont.descender + Self.nameFont.leading)
        let countHeight = ceil(Self.countFont.ascender - Self.countFont.descender + Self.countFont.leading)
        tracks.frame = CGRect(x: bounds.width - Self.countWidth, y: (height - countHeight) / 2, width: Self.countWidth, height: countHeight)
        albums.frame = CGRect(x: tracks.frame.minX - Self.countWidth, y: tracks.frame.minY, width: Self.countWidth, height: countHeight)

        let nameX: CGFloat = 28
        let room = albums.frame.minX - 12 - 26 - nameX
        let wanted = ceil((name.stringValue as NSString).size(withAttributes: [.font: Self.nameFont]).width) + 5
        name.frame = CGRect(x: nameX, y: (height - lineHeight) / 2, width: min(wanted, max(room, 0)), height: lineHeight)

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        heart.frame = Symbol.frame(of: heartImage, centredIn: CGRect(x: name.frame.maxX + 10, y: 0, width: 16, height: height))
        CATransaction.commit()
    }

    // MARK: Pointer

    private func part(at point: NSPoint) -> Part {
        if point.x < 22 { return .mark }
        if name.frame.contains(point) { return .name }
        if heart.frame.insetBy(dx: -3, dy: -3).contains(point) { return .heart }
        return .elsewhere
    }

    func hover(at point: NSPoint?) -> Bool {
        let now = point.map(part(at:))
        if now != hovered {
            hovered = now
            restyle()
        }
        return now == .name
    }

    func hit(at point: NSPoint) -> RowHit {
        guard let artist, let context else { return .plain }
        switch part(at: point) {
        case .mark: return .button { [weak self] in _ = self?.play() }
        case .heart: return .button { context.toggleFavourite(artist.id) }
        case .name: return .link { context.open(artist.id) }
        case .elsewhere: return .plain
        }
    }

    @discardableResult
    private func play() -> Bool {
        guard let artist, let context, !playing else { return false }
        playing = true
        restyle()
        Task { [weak self] in
            await context.play(artist)
            guard let self, self.artist?.id == artist.id else { return }
            self.playing = false
            self.restyle()
        }
        return true
    }
}
#endif
