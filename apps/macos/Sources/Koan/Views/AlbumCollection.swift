#if os(macOS)
import AppKit
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

/// The album grid on the Mac: an `NSCollectionView` of tiles made of layers
/// and labels. iOS, and the handful of records on an artist's page, use
/// `AlbumGridCell` in a `LazyVGrid`.
///
/// SwiftUI's grid builds each tile afresh as it scrolls in and places, diffs
/// and hit-tests every view of every tile on screen on each scroll step. On a
/// large display that took 22–27 ms a step, more than a 60 Hz frame. A
/// collection view reuses a fixed pool of tiles and moves them, and the same
/// scroll took 11–12 ms.
///
/// Tiles are layers and labels. On macOS 26 every AppKit control — a button,
/// an image view, a spinner, glass — is a SwiftUI view graph of its own, and
/// one per tile brought the cost straight back; the few a tile needs are made
/// only while they are showing.
///
/// Behaviour follows `AlbumGridCell`: the sleeve plays the record and opens it,
/// the title opens it, the artist name links out; hover shows the play
/// affordance and the heart; ⌘-click starts a pick and, while picking, a click
/// anywhere on a tile ticks it. The context menu is the SwiftUI
/// `PlayableMenu`, hosted.
struct AlbumCollection: NSViewRepresentable {
    let albums: [Album]
    let selection: PlayableSelection
    /// What the tiles show that can change under them, passed in so a change
    /// arrives as an update rather than being observed tile by tile.
    let picked: Set<Playable.Key>
    let selecting: Bool
    let favourites: Set<Int64>
    let tint: Color
    let usesGlass: Bool
    /// The toolbar above and the transport below, which the grid scrolls
    /// under rather than stopping at.
    let insets: EdgeInsets
    /// Bumped to send the grid back to its top.
    let rewinds: Int
    let actions: AlbumTile.Actions

    @Environment(LibraryModel.self) private var library
    @Environment(CoverArtCache.self) private var art

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let grid = AlbumGridView()
        grid.collectionViewLayout = TileLayout()
        grid.isSelectable = false
        grid.backgroundColors = [.clear]
        grid.register(AlbumTile.self, forItemWithIdentifier: AlbumTile.identifier)

        let scroll = NSScrollView()
        scroll.documentView = grid
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.contentView.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false

        context.coordinator.attach(grid: grid, scroll: scroll, library: library)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let insets = NSEdgeInsets(top: self.insets.top, left: 0, bottom: self.insets.bottom, right: 0)
        if scroll.contentInsets.top != insets.top || scroll.contentInsets.bottom != insets.bottom {
            scroll.contentInsets = insets
            // The scroller runs up under the toolbar, as a SwiftUI scroll
            // view's does, and stops above the transport.
            scroll.scrollerInsets = NSEdgeInsets(top: 0, left: 0, bottom: insets.bottom, right: 0)
        }
        // The sidebar floats over the page's leading edge. The grid scrolls
        // under it, as the toolbar, but its first column starts clear of it.
        if let grid = scroll.documentView as? NSCollectionView,
           let layout = grid.collectionViewLayout as? TileLayout,
           layout.leading != self.insets.leading {
            layout.leading = self.insets.leading
        }

        let coordinator = context.coordinator
        // Everything the page was handed, for the menu: it reads the same
        // models it would in a SwiftUI grid. Taken from the context rather
        // than read with `@Environment(\.self)`, which would make the grid
        // depend on every value in the environment.
        let environment = context.environment
        coordinator.context = AlbumTile.Context(
            art: art,
            selection: selection,
            picked: picked,
            selecting: selecting,
            favourites: favourites,
            tint: NSColor(tint),
            usesGlass: usesGlass,
            actions: actions,
            menu: { album in
                NSHostingMenu(rootView: PlayableMenu(playable: .album(album))
                    .transformEnvironment(\.self) { $0 = environment })
            }
        )
        coordinator.show(albums)
        coordinator.rewind(to: rewinds)
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.detach()
    }

    @MainActor
    final class Coordinator: NSObject {
        var context: AlbumTile.Context?
        private weak var grid: AlbumGridView?
        private weak var scroll: NSScrollView?
        private weak var library: LibraryModel?
        private var source: NSCollectionViewDiffableDataSource<Int, Int64>?
        private var albums: [Int64: Album] = [:]
        private var order: [Int64] = []
        private var rewinds: Int?
        private var restored = false
        private var watching: (any NSObjectProtocol)?
        private var shownState: TileState?

        /// What the tiles on screen were last brought up to date with.
        private struct TileState: Equatable {
            let picked: Set<Playable.Key>
            let selecting: Bool
            let favourites: Set<Int64>
            let tint: NSColor
            let usesGlass: Bool

            init?(_ context: AlbumTile.Context?) {
                guard let context else { return nil }
                picked = context.picked
                selecting = context.selecting
                favourites = context.favourites
                tint = context.tint
                usesGlass = context.usesGlass
            }
        }

        func attach(grid: AlbumGridView, scroll: NSScrollView, library: LibraryModel) {
            self.grid = grid
            grid.laidOut = { [weak self] in self?.restoreOnce() }
            self.scroll = scroll
            self.library = library
            source = NSCollectionViewDiffableDataSource(collectionView: grid) { [weak self] grid, path, id in
                let item = grid.makeItem(withIdentifier: AlbumTile.identifier, for: path)
                if let tile = item as? AlbumTile, let self, let album = self.albums[id], let context = self.context {
                    tile.show(album, in: context)
                }
                return item
            }
            // Where the grid is scrolled to, as a distance — see
            // `LibraryModel.albumsOffset`. A reshuffle reorders every record,
            // and a distance is what it leaves alone.
            scroll.contentView.postsBoundsChangedNotifications = true
            watching = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.noteOffset()
                    self?.grid?.pointerMayHaveMoved()
                }
            }
        }

        func detach() {
            if let watching { NotificationCenter.default.removeObserver(watching) }
        }

        func show(_ albums: [Album]) {
            let state = TileState(context)
            let restyled = state != shownState
            shownState = state
            let ids = albums.map(\.id)
            let changed = albums.filter { self.albums[$0.id] != $0 }.map(\.id)
            self.albums = Dictionary(albums.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            if ids != order {
                order = ids
                var snapshot = NSDiffableDataSourceSnapshot<Int, Int64>()
                snapshot.appendSections([0])
                // A snapshot traps on a repeated id; a record listed twice
                // shows once.
                var seen = Set<Int64>()
                snapshot.appendItems(ids.filter { seen.insert($0).inserted })
                source?.apply(snapshot, animatingDifferences: false)
            }
            if restyled || !changed.isEmpty { refreshVisible(reshow: Set(changed)) }
        }

        func rewind(to count: Int) {
            defer { rewinds = count }
            guard let rewinds, rewinds != count, let scroll else { return }
            scroll.contentView.scroll(to: NSPoint(x: 0, y: -scroll.contentInsets.top))
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        /// Every tile on screen, brought up to date with what changed around
        /// it: a tick, a heart, the room's colour. Tiles off screen catch up
        /// when they are next shown.
        private func refreshVisible(reshow: Set<Int64>) {
            guard let grid, let context else { return }
            for path in grid.indexPathsForVisibleItems() {
                guard let tile = grid.item(at: path) as? AlbumTile,
                      let id = source?.itemIdentifier(for: path),
                      let album = albums[id]
                else { continue }
                if reshow.contains(id) {
                    tile.show(album, in: context)
                } else {
                    tile.update(in: context)
                }
            }
        }

        /// Back to where the grid was left, once there is a grid to scroll:
        /// the first layout with the records in it.
        private func restoreOnce() {
            guard !restored, !order.isEmpty, let scroll else { return }
            restored = true
            guard let offset = library?.albumsOffset else { return }
            scroll.contentView.scroll(to: NSPoint(x: 0, y: offset - scroll.contentInsets.top))
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        private func noteOffset() {
            guard restored, let scroll else { return }
            library?.albumsOffset = scroll.contentView.bounds.minY + scroll.contentInsets.top
        }
    }
}

/// A collection view that says when it has laid out, which is when a
/// remembered scroll position can be put back.
final class AlbumGridView: NSCollectionView {
    var laidOut: (() -> Void)?
    private var hovered: AlbumTile?
    private var linked = false

    override func layout() {
        super.layout()
        laidOut?()
    }

    // Hover for every tile from one tracking area. One per tile is a
    // tracking area per tile for AppKit to move on every scroll step.

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard trackingAreas.isEmpty else { return }
        addTrackingArea(NSTrackingArea(
            rect: .zero,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self
        ))
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        hover(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseEntered(with event: NSEvent) {
        super.mouseEntered(with: event)
        hover(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        hover(at: nil)
    }

    /// The records move under a pointer that stays still while the grid
    /// scrolls, so where it rests is asked again then.
    func pointerMayHaveMoved() {
        guard let window, window.isKeyWindow else { return hover(at: nil) }
        let point = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        hover(at: visibleRect.contains(point) ? point : nil)
    }

    private func hover(at point: NSPoint?) {
        let tile = point
            .flatMap { indexPathForItem(at: $0) }
            .flatMap { item(at: $0) as? AlbumTile }
        if tile !== hovered {
            hovered?.hoverEnded()
            hovered = tile
        }
        let overLink = tile.map { tile in
            tile.hover(at: point.map { tile.view.convert($0, from: self) })
        } ?? false
        if overLink != linked {
            linked = overLink
            (overLink ? NSCursor.pointingHand : NSCursor.arrow).set()
        }
    }
}

/// Columns of at least 150 points sharing the width, as the adaptive
/// `GridItem` on the other platforms lays them out. Every tile is the same
/// size, so nothing is measured.
private final class TileLayout: NSCollectionViewFlowLayout {
    private let minimum: CGFloat = 150
    private let maximum: CGFloat = 210
    private let margin: CGFloat = 20

    /// What floats over the grid's leading edge.
    var leading: CGFloat = 0 {
        didSet {
            sectionInset.left = margin + leading
            invalidateLayout()
        }
    }

    override init() {
        super.init()
        minimumInteritemSpacing = 18
        minimumLineSpacing = 22
        sectionInset = NSEdgeInsets(top: margin, left: margin, bottom: margin, right: margin)
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override func prepare() {
        if let width = collectionView?.bounds.width {
            let usable = width - sectionInset.left - sectionInset.right
            let columns = max(1, ((usable + minimumInteritemSpacing) / (minimum + minimumInteritemSpacing)).rounded(.down))
            let side = min(maximum, ((usable - minimumInteritemSpacing * (columns - 1)) / columns).rounded(.down))
            itemSize = NSSize(width: side, height: side + AlbumTile.captionHeight)
        }
        super.prepare()
    }

    override func shouldInvalidateLayout(forBoundsChange newBounds: NSRect) -> Bool {
        newBounds.width != collectionView?.bounds.width
    }
}

/// One record: the sleeve, its title, who made it and when.
final class AlbumTile: NSCollectionViewItem {
    static let identifier = NSUserInterfaceItemIdentifier("AlbumTile")

    struct Actions {
        var open: (Int64) -> Void = { _ in }
        var openArtist: (Int64) -> Void = { _ in }
        /// Resolves the record's tracks and plays them. Returns once they are
        /// playing, so the tile can show it is busy until then.
        var play: (Int64) async -> Void = { _ in }
        var toggleFavourite: (Int64) -> Void = { _ in }
    }

    /// What every tile is shown in.
    struct Context {
        let art: CoverArtCache
        let selection: PlayableSelection
        let picked: Set<Playable.Key>
        let selecting: Bool
        let favourites: Set<Int64>
        let tint: NSColor
        let usesGlass: Bool
        let actions: Actions
        let menu: (Album) -> NSMenu
    }

    private enum Part { case sleeve, title, artist, elsewhere }

    private static let titleFont = NSFont.systemFont(
        ofSize: NSFont.preferredFont(forTextStyle: .callout).pointSize, weight: .medium
    )
    private static let detailFont = NSFont.preferredFont(forTextStyle: .caption1)
    /// The gap between sleeve, title and credit, as `AlbumGridCell`'s stack.
    private static let gap: CGFloat = 7
    private static let titleHeight = lineHeight(titleFont)
    private static let detailHeight = lineHeight(detailFont)

    static var captionHeight: CGFloat { gap + titleHeight + gap + detailHeight }

    private static func width(of credit: String) -> CGFloat {
        guard !credit.isEmpty else { return 0 }
        // Labels pad their text by a couple of points either side.
        return ceil((credit as NSString).size(withAttributes: [.font: detailFont]).width) + 5
    }

    private static func lineHeight(_ font: NSFont) -> CGFloat {
        ceil(font.ascender - font.descender + font.leading)
    }

    // The sleeve and what sits on it.
    private let shade = CALayer()
    private let sleeve = CALayer()
    private let ensō = CAShapeLayer()
    private let dim = CALayer()
    private let playMark = CALayer()
    /// Made while the sleeve is loading or the record starting, and only
    /// then. Every AppKit control is a SwiftUI view graph on macOS 26, so one
    /// per tile is a graph per tile; layers and labels are not.
    private var spinner: NSProgressIndicator?
    private let badge = CALayer()
    private let codec = CATextLayer()
    private static let codecFont = NSFont.monospacedSystemFont(ofSize: 9, weight: .semibold)
    /// Made while there is a heart to show: the tile is hovered or the
    /// record a favourite. See `spinner`.
    private var heart: HeartButton?
    private let ring = CALayer()
    private let tick = CALayer()

    // The caption.
    private let titleLabel = NSTextField(labelWithString: "")
    private let artistLabel = NSTextField(labelWithString: "")
    private let yearLabel = NSTextField(labelWithString: "")

    private var album: Album?
    private var context: Context?
    private var load: Task<Void, Never>?
    private var playing = false
    private var hovered: Part?
    private var underlined: (title: Bool, artist: Bool) = (false, false)
    /// The sleeve on show, for the drag image.
    private var shown: CGImage? {
        didSet { sleeve.contents = shown }
    }

    override func loadView() {
        let root = TileView()
        root.tile = self
        root.wantsLayer = true
        guard let layer = root.layer else { return }

        shade.shadowOpacity = 0.28
        shade.shadowRadius = 7
        shade.shadowOffset = CGSize(width: 0, height: 3)
        layer.addSublayer(shade)

        sleeve.cornerRadius = 6
        sleeve.cornerCurve = .continuous
        sleeve.masksToBounds = true
        sleeve.contentsGravity = .resizeAspectFill
        sleeve.borderWidth = 1
        layer.addSublayer(sleeve)

        ensō.fillColor = nil
        ensō.lineCap = .round
        ensō.lineJoin = .round
        ensō.opacity = 0.5
        sleeve.addSublayer(ensō)

        dim.backgroundColor = NSColor.black.withAlphaComponent(0.35).cgColor
        dim.opacity = 0
        sleeve.addSublayer(dim)

        playMark.contents = Symbol.image("play.circle.fill", size: 34, colours: [.white])
        playMark.isHidden = true
        playMark.shadowOpacity = 0.33
        playMark.shadowRadius = 4
        playMark.shadowOffset = .zero
        layer.addSublayer(playMark)

        // A scrim rather than the clear glass the SwiftUI tile wears: AppKit's
        // glass view is a SwiftUI view graph of its own, and one per tile was
        // re-rendered on every scroll step, which cost more than the whole
        // grid.
        badge.backgroundColor = NSColor.black.withAlphaComponent(0.45).cgColor
        badge.cornerCurve = .continuous
        codec.font = Self.codecFont
        codec.fontSize = Self.codecFont.pointSize
        codec.foregroundColor = NSColor.white.cgColor
        codec.alignmentMode = .center
        badge.addSublayer(codec)
        layer.addSublayer(badge)

        ring.cornerRadius = 6
        ring.cornerCurve = .continuous
        ring.borderWidth = 3
        ring.isHidden = true
        layer.addSublayer(ring)

        tick.isHidden = true
        tick.shadowOpacity = 0.35
        tick.shadowRadius = 2
        tick.shadowOffset = .zero
        layer.addSublayer(tick)

        for label in [titleLabel, artistLabel, yearLabel] {
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            label.cell?.truncatesLastVisibleLine = true
            root.addSubview(label)
        }
        titleLabel.font = Self.titleFont
        titleLabel.textColor = .labelColor
        artistLabel.font = Self.detailFont
        artistLabel.textColor = .secondaryLabelColor
        yearLabel.font = Self.detailFont
        yearLabel.textColor = .secondaryLabelColor

        view = root
        applyColours()
    }

    // MARK: - Showing a record

    func show(_ album: Album, in context: Context) {
        let changedRecord = self.album?.id != album.id
        self.album = album
        self.context = context
        titleLabel.stringValue = album.title
        artistLabel.stringValue = album.artistName
        artistLabel.toolTip = "Go to \(album.artistName)"
        yearLabel.stringValue = album.year.map { "· \($0)" } ?? ""
        codec.string = album.codec?.uppercased()
        badge.isHidden = album.codec == nil
        view.setAccessibilityLabel([album.title, album.artistName, album.year.map(String.init)].compactMap { $0 }.joined(separator: ", "))
        view.setAccessibilityRole(.group)
        // What the SwiftUI tile's buttons offered VoiceOver.
        view.setAccessibilityCustomActions([
            NSAccessibilityCustomAction(name: "Play") { [weak self] in self?.act(.sleeve) ?? false },
            NSAccessibilityCustomAction(name: "Open album") { [weak self] in self?.act(.title) ?? false },
            NSAccessibilityCustomAction(name: "Go to \(album.artistName)") { [weak self] in self?.act(.artist) ?? false },
        ])
        if changedRecord {
            playing = false
            hovered = nil
            showSleeve(album.id, art: context.art)
        }
        update(in: context)
        view.needsLayout = true
    }

    /// The parts that follow the page rather than the record: the pick, the
    /// favourites, hover, the room's colour.
    func update(in context: Context) {
        self.context = context
        guard let album else { return }
        let key = Playable.album(album).key
        let selected = context.picked.contains(key)

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        ring.isHidden = !(context.selecting && selected)
        ring.borderColor = context.tint.cgColor
        tick.isHidden = !context.selecting
        if context.selecting {
            tick.contents = selected
                ? Symbol.image("checkmark.circle.fill", size: 20, colours: [.white, context.tint])
                : Symbol.image("circle", size: 20, colours: [.white, .black.withAlphaComponent(0.25)])
        }
        CATransaction.commit()

        let favourite = context.favourites.contains(album.id)
        showHeart(!context.selecting && (favourite || hovered != nil), on: favourite, glassy: context.usesGlass)

        let showsPlay = !context.selecting && (hovered == .sleeve || playing)
        CATransaction.begin()
        CATransaction.setAnimationDuration(0.12)
        dim.opacity = showsPlay ? 1 : 0
        CATransaction.commit()
        playMark.isHidden = !showsPlay || playing
        if playing { spin(true) }

        let linked = !context.selecting && hovered == .artist
        let titled = !context.selecting && hovered == .title
        if underlined != (titled, linked) {
            underlined = (titled, linked)
            artistLabel.textColor = linked ? .labelColor : .secondaryLabelColor
            underline(artistLabel, linked)
            underline(titleLabel, titled)
        }
    }

    private func showHeart(_ wanted: Bool, on: Bool, glassy: Bool) {
        if let heart, !wanted || heart.glassy != glassy {
            heart.removeFromSuperview()
            self.heart = nil
        }
        if wanted, heart == nil {
            let made = HeartButton(glassy: glassy)
            made.target = self
            made.action = #selector(toggleFavourite)
            view.addSubview(made)
            heart = made
            view.needsLayout = true
        }
        heart?.isOn = on
    }

    private func spin(_ on: Bool) {
        if on, spinner == nil {
            let made = NSProgressIndicator()
            made.style = .spinning
            made.controlSize = .small
            view.addSubview(made)
            made.startAnimation(nil)
            spinner = made
            view.needsLayout = true
        } else if !on, let spinner {
            spinner.stopAnimation(nil)
            spinner.removeFromSuperview()
            self.spinner = nil
        }
    }

    private func showSleeve(_ id: Int64, art: CoverArtCache) {
        load?.cancel()
        sleeve.removeAnimation(forKey: "contents")
        if let held = art.cached(.album(id), size: .tile) {
            shown = held.bitmap
            ensō.isHidden = true
            if !playing { spin(false) }
            return
        }
        shown = nil
        ensō.isHidden = false
        // Settle first, as `AlbumArtwork` does: flying past a sleeve should
        // not fetch it.
        load = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(180))
            guard !Task.isCancelled, let self else { return }
            self.ensō.isHidden = true
            self.spin(true)
            let image = await art.image(for: .album(id), size: .tile)
            guard !Task.isCancelled, self.album?.id == id else { return }
            if !self.playing { self.spin(false) }
            guard let bitmap = image?.bitmap else {
                self.ensō.isHidden = false
                return
            }
            let fade = CABasicAnimation(keyPath: "contents")
            fade.duration = 0.2
            self.sleeve.add(fade, forKey: "contents")
            self.shown = bitmap
        }
    }

    private func underline(_ label: NSTextField, _ on: Bool) {
        let text = label.stringValue
        var attributes: [NSAttributedString.Key: Any] = [
            .font: label.font as Any,
            .foregroundColor: label.textColor as Any,
        ]
        if on { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingTail
        attributes[.paragraphStyle] = paragraph
        label.attributedStringValue = NSAttributedString(string: text, attributes: attributes)
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        load?.cancel()
        load = nil
        hovered = nil
        spin(false)
        showHeart(false, on: false, glassy: true)
    }

    // MARK: - Layout

    override func viewDidLayout() {
        super.viewDidLayout()
        let side = view.bounds.width
        let art = CGRect(x: 0, y: 0, width: side, height: side)

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        shade.frame = art
        shade.shadowPath = CGPath(roundedRect: CGRect(origin: .zero, size: art.size), cornerWidth: 6, cornerHeight: 6, transform: nil)
        sleeve.frame = art
        dim.frame = sleeve.bounds
        ring.frame = art
        let inset = side * 0.26
        ensō.frame = sleeve.bounds
        ensō.lineWidth = side * 0.045
        ensō.path = EnsoShape().path(in: CGRect(x: inset, y: inset, width: side - 2 * inset, height: side - 2 * inset)).cgPath

        playMark.frame = Symbol.frame(of: Symbol.image("play.circle.fill", size: 34, colours: [.white]), centredIn: art)
        tick.frame = Symbol.frame(of: Symbol.image("circle", size: 20, colours: [.white, .black.withAlphaComponent(0.25)]), at: CGPoint(x: 7, y: 7))

        let text = codec.string as? String ?? ""
        let textWidth = ceil((text as NSString).size(withAttributes: [.font: Self.codecFont]).width)
        let textHeight = Self.lineHeight(Self.codecFont)
        badge.frame = CGRect(x: art.maxX - 6 - (textWidth + 12), y: 6, width: textWidth + 12, height: textHeight + 4)
        badge.cornerRadius = badge.frame.height / 2
        codec.frame = CGRect(x: 6, y: 2, width: textWidth, height: textHeight)
        codec.contentsScale = view.window?.backingScaleFactor ?? 2
        CATransaction.commit()

        spinner?.frame = CGRect(x: art.midX - 8, y: art.midY - 8, width: 16, height: 16)
        heart?.frame = CGRect(x: art.maxX - 7 - 30, y: art.maxY - 7 - 30, width: 30, height: 30)

        let titleY = art.maxY + Self.gap
        titleLabel.frame = CGRect(x: 0, y: titleY, width: side, height: Self.titleHeight)
        let detailY = titleY + Self.titleHeight + Self.gap
        let year = Self.width(of: yearLabel.stringValue)
        let artist = min(Self.width(of: artistLabel.stringValue), side - year)
        artistLabel.frame = CGRect(x: 0, y: detailY, width: artist, height: Self.detailHeight)
        yearLabel.frame = CGRect(x: artist, y: detailY, width: year, height: Self.detailHeight)
    }

    // MARK: - Pointer

    private func part(at point: NSPoint) -> Part {
        if sleeve.frame.contains(point) { return .sleeve }
        if titleLabel.frame.contains(point) { return .title }
        if artistLabel.frame.contains(point) { return .artist }
        return .elsewhere
    }

    /// The pointer is at `point` in the tile, or has left it. Whether it is
    /// over a link, for the cursor.
    @discardableResult
    fileprivate func hover(at point: NSPoint?) -> Bool {
        let now = point.map(part(at:))
        if now != hovered, let context {
            hovered = now
            update(in: context)
        }
        return now == .artist && context?.selecting == false
    }

    fileprivate func hoverEnded() { hover(at: nil) }

    fileprivate func clicked(at point: NSPoint) {
        guard let album, let context else { return }
        // A tick while picking, or a new pick on ⌘-click — the way
        // `AlbumGridCell` takes clicks before anything else gets them.
        if context.selection.take(.album(album)) { return }
        act(part(at: point))
    }

    @discardableResult
    private func act(_ part: Part) -> Bool {
        guard let album, let context else { return false }
        switch part {
        case .sleeve: play(album, context: context)
        case .title: context.actions.open(album.id)
        case .artist: context.actions.openArtist(album.artistId)
        case .elsewhere: return false
        }
        return true
    }

    private func play(_ album: Album, context: Context) {
        guard !playing else { return }
        playing = true
        update(in: context)
        Task { [weak self] in
            await context.actions.play(album.id)
            guard let self, self.album?.id == album.id else { return }
            self.playing = false
            self.spin(false)
            if let context = self.context { self.update(in: context) }
        }
    }

    @objc private func toggleFavourite() {
        guard let album, let context else { return }
        if context.selection.take(.album(album)) { return }
        context.actions.toggleFavourite(album.id)
    }

    fileprivate func contextMenu() -> NSMenu? {
        guard let album, let context else { return nil }
        return context.menu(album)
    }

    // MARK: - Drag

    /// What a drag from `point` carries: the artist from its name, otherwise
    /// the record — or the whole pick, when the record is part of it.
    fileprivate func dragged(from point: NSPoint) -> [PlayableTransfer] {
        guard let album, let context else { return [] }
        if part(at: point) == .artist, !context.selecting {
            return [PlayableTransfer(kind: .artist, id: album.artistId, name: album.artistName)]
        }
        let key = Playable.album(album).key
        if context.selecting, context.picked.contains(key) {
            return context.selection.picked.map(PlayableTransfer.init)
        }
        return [PlayableTransfer(.album(album))]
    }

    fileprivate var dragImage: NSImage? {
        shown.map { NSImage(cgImage: $0, size: NSSize(width: 64, height: 64)) }
    }

    // MARK: - Appearance

    fileprivate func applyColours() {
        view.effectiveAppearance.performAsCurrentDrawingAppearance {
            shade.shadowColor = NSColor.black.cgColor
            sleeve.backgroundColor = NSColor.quaternaryLabelColor.cgColor
            sleeve.borderColor = NSColor.white.withAlphaComponent(0.06).cgColor
            ensō.strokeColor = NSColor.tertiaryLabelColor.cgColor
        }
    }
}

/// SF Symbols drawn once into bitmaps for layers, which take no part in
/// layout or hit-testing.
@MainActor
private enum Symbol {
    private static var cache: [String: CGImage] = [:]

    static func image(_ name: String, size: CGFloat, colours: [NSColor]) -> CGImage? {
        let key = "\(name) \(size) \(colours.map(\.description))"
        if let held = cache[key] { return held }
        let configuration = NSImage.SymbolConfiguration(pointSize: size, weight: .regular)
            .applying(.init(paletteColors: colours))
        guard let symbol = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(configuration),
            let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: Int(ceil(symbol.size.width * 2)), pixelsHigh: Int(ceil(symbol.size.height * 2)),
                bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
            )
        else { return nil }
        bitmap.size = symbol.size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        symbol.draw(in: NSRect(origin: .zero, size: symbol.size))
        NSGraphicsContext.restoreGraphicsState()
        cache[key] = bitmap.cgImage
        return bitmap.cgImage
    }

    /// Where a symbol drawn by `image` sits at its natural size: it is drawn
    /// at 2×, so half its pixels.
    static func frame(of image: CGImage?, centredIn rect: CGRect) -> CGRect {
        let size = size(of: image)
        return CGRect(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2, width: size.width, height: size.height)
    }

    static func frame(of image: CGImage?, at origin: CGPoint) -> CGRect {
        CGRect(origin: origin, size: size(of: image))
    }

    private static func size(of image: CGImage?) -> CGSize {
        guard let image else { return .zero }
        return CGSize(width: CGFloat(image.width) / 2, height: CGFloat(image.height) / 2)
    }
}

/// The tile's own view: it takes the clicks, drags, hover and menu for the
/// tile, and is flipped so the tile lays out top down.
private final class TileView: NSView, NSDraggingSource {
    weak var tile: AlbumTile?
    private var pressedAt: NSPoint?

    override var isFlipped: Bool { true }

    override func mouseDown(with event: NSEvent) { pressedAt = point(event) }

    override func mouseDragged(with event: NSEvent) {
        guard let start = pressedAt, let tile else { return }
        let now = point(event)
        guard hypot(now.x - start.x, now.y - start.y) > 3 else { return }
        pressedAt = nil
        let transfers = tile.dragged(from: start)
        let image = tile.dragImage
        let items = transfers.compactMap { transfer -> NSDraggingItem? in
            guard let data = try? JSONEncoder().encode(transfer) else { return nil }
            let item = NSPasteboardItem()
            item.setData(data, forType: NSPasteboard.PasteboardType(UTType.koanPlayable.identifier))
            item.setString(transfer.name, forType: .string)
            let dragging = NSDraggingItem(pasteboardWriter: item)
            dragging.setDraggingFrame(NSRect(x: start.x - 32, y: start.y - 32, width: 64, height: 64), contents: image)
            return dragging
        }
        guard !items.isEmpty else { return }
        beginDraggingSession(with: items, event: event, source: self)
    }

    override func mouseUp(with event: NSEvent) {
        defer { pressedAt = nil }
        guard pressedAt != nil, event.clickCount == 1 else { return }
        tile?.clicked(at: point(event))
    }

    override func menu(for event: NSEvent) -> NSMenu? { tile?.contextMenu() }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        tile?.applyColours()
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    private func point(_ event: NSEvent) -> NSPoint {
        convert(event.locationInWindow, from: nil)
    }
}

/// The heart on a tile: on a ground of clear glass, as the other platforms'
/// `AlbumTileHeart`, so it reads over any sleeve. A flat material instead
/// where the Graphics setting turns glass off, as `AffordableGlass`.
private final class HeartButton: NSButton {
    let glassy: Bool
    private let ground: NSView
    private let glyph = NSImageView()

    var isOn = false {
        didSet {
            guard isOn != oldValue || glyph.image == nil else { return }
            let name = isOn ? "heart.fill" : "heart"
            glyph.image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 13, weight: .regular))
            glyph.contentTintColor = isOn ? .systemRed : .tertiaryLabelColor
            setAccessibilityLabel(isOn ? "Remove favourite" : "Favourite")
            toolTip = isOn ? "Remove favourite" : "Favourite"
            if oldValue != isOn, window != nil {
                glyph.addSymbolEffect(.bounce.up.byLayer, options: .speed(1.4))
            }
        }
    }

    init(glassy: Bool) {
        self.glassy = glassy
        if glassy {
            let glass = NSGlassEffectView()
            glass.style = .clear
            glass.contentView = glyph
            ground = glass
        } else {
            let flat = NSVisualEffectView()
            flat.material = .hudWindow
            flat.blendingMode = .withinWindow
            flat.wantsLayer = true
            flat.addSubview(glyph)
            ground = flat
        }
        super.init(frame: .zero)
        isBordered = false
        title = ""
        addSubview(ground)
        isOn = false
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override func layout() {
        super.layout()
        ground.frame = bounds
        glyph.frame = bounds
        if let glass = ground as? NSGlassEffectView {
            glass.cornerRadius = bounds.height / 2
        } else {
            ground.layer?.cornerRadius = bounds.height / 2
        }
    }
}
#endif
