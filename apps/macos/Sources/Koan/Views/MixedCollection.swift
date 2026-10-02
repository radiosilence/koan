#if os(macOS)
import AppKit
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

/// A page of artists, records and tracks together, on the Mac: favourites.
///
/// One collection view in three sections: the artists as pills, the records
/// as the album grid's tiles, the tracks as the track lists' rows. Each is
/// made only as it scrolls in and reused after, so the page costs what is on
/// screen — a `List` with a flow of pills and a grid riding above its rows
/// built every one of them before it drew. Only the tracks select, the way
/// they did in the `List`: range-select, Return to play, a menu on the pick.
struct MixedCollection: NSViewRepresentable {
    let artists: [Artist]
    let albums: [Album]
    let tracks: [TrackLine]
    let tileContext: AlbumTile.Context
    let trackContext: TrackTableRow.Context
    /// Changes whenever something the items draw changes.
    let contextKey: AnyHashable
    @Binding var selection: Set<Int64>
    var albumMenu: (Album, EnvironmentValues) -> NSMenu = { _, _ in NSMenu() }
    var artistMenu: (Artist, EnvironmentValues) -> NSMenu? = { _, _ in nil }
    var trackMenu: (Set<Int64>, EnvironmentValues) -> NSMenu? = { _, _ in nil }
    let openArtist: (Int64) -> Void
    /// Double-click or Return on tracks.
    let primaryAction: (Set<Int64>) -> Void
    /// Whether the tracks are a list to select from, as favourites, or
    /// results that go somewhere when clicked, as search.
    var tracksSelect = true
    /// A click on a result track.
    var openTrack: (Track) -> Void = { _ in }
    /// The page's pick, where everything on it takes part in one — see
    /// `PlayableSelection`.
    var pick: PlayableSelection?
    /// How many are in each section, beside its title.
    var counts = false
    var selectAllToken = 0
    let insets: EdgeInsets

    fileprivate enum Section: Int, CaseIterable {
        case artists, albums, tracks

        var title: String {
            switch self {
            case .artists: "Artists"
            case .albums: "Albums"
            case .tracks: "Tracks"
            }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let collection = MixedCollectionView()
        collection.collectionViewLayout = MixedLayout()
        collection.isSelectable = true
        collection.allowsMultipleSelection = true
        collection.backgroundColors = [.clear]
        collection.register(ArtistPillItem.self, forItemWithIdentifier: ArtistPillItem.identifier)
        collection.register(AlbumTile.self, forItemWithIdentifier: AlbumTile.identifier)
        collection.register(TrackItem.self, forItemWithIdentifier: TrackItem.identifier)
        collection.register(
            SectionHeader.self,
            forSupplementaryViewOfKind: NSCollectionView.elementKindSectionHeader,
            withIdentifier: SectionHeader.identifier
        )
        collection.dataSource = context.coordinator
        collection.delegate = context.coordinator
        collection.owner = context.coordinator
        collection.setDraggingSourceOperationMask(.copy, forLocal: true)
        collection.setDraggingSourceOperationMask(.copy, forLocal: false)

        let scroll = NSScrollView()
        scroll.documentView = collection
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.contentView.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false
        context.coordinator.attach(collection, scroll: scroll)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let content = NSEdgeInsets(top: insets.top, left: 0, bottom: insets.bottom, right: 0)
        let current = scroll.contentInsets
        if current.top != content.top || current.bottom != content.bottom {
            scroll.contentInsets = content
            scroll.scrollerInsets = NSEdgeInsets(top: 0, left: 0, bottom: insets.bottom, right: 0)
        }
        if let layout = (scroll.documentView as? NSCollectionView)?.collectionViewLayout as? MixedLayout,
           layout.leading != insets.leading {
            layout.leading = insets.leading
        }
        context.coordinator.update(self, environment: context.environment)
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.detach()
    }

    @MainActor
    final class Coordinator: NSObject, NSCollectionViewDataSource, NSCollectionViewDelegate, MixedLayoutSource {
        fileprivate var parent: MixedCollection?
        fileprivate var environment = EnvironmentValues()
        private weak var collection: MixedCollectionView?
        private weak var scroll: NSScrollView?
        private var shown: (artists: [Int64], albums: [Int64], tracks: [Int64]) = ([], [], [])
        private var shownKey: AnyHashable?
        private var selectAllToken: Int?
        private var applying = false
        private var watching: (any NSObjectProtocol)?

        fileprivate var sections: [Section] {
            guard let parent else { return [] }
            return Section.allCases.filter { section in
                switch section {
                case .artists: !parent.artists.isEmpty
                case .albums: !parent.albums.isEmpty
                case .tracks: !parent.tracks.isEmpty
                }
            }
        }

        func attach(_ collection: MixedCollectionView, scroll: NSScrollView) {
            self.collection = collection
            self.scroll = scroll
            (collection.collectionViewLayout as? MixedLayout)?.source = self
            scroll.contentView.postsBoundsChangedNotifications = true
            watching = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.collection?.pointerMayHaveMoved() }
            }
        }

        func detach() {
            if let watching { NotificationCenter.default.removeObserver(watching) }
        }

        func update(_ parent: MixedCollection, environment: EnvironmentValues) {
            self.parent = parent
            self.environment = environment
            guard let collection else { return }
            let now = (parent.artists.map(\.id), parent.albums.map(\.id), parent.tracks.map(\.id))
            if now != shown {
                shown = now
                shownKey = parent.contextKey
                collection.reloadData()
            } else if parent.contextKey != shownKey {
                shownKey = parent.contextKey
                for item in collection.visibleItems() {
                    guard let path = collection.indexPath(for: item) else { continue }
                    configure(item, at: path)
                }
            }
            applySelection(parent.selection)
            if let token = selectAllToken, token != parent.selectAllToken,
               collection.window?.firstResponder === collection,
               let section = sections.firstIndex(of: .tracks) {
                let all = Set((0..<parent.tracks.count).map { IndexPath(item: $0, section: section) })
                collection.selectionIndexPaths = all
                selectionChanged()
            }
            selectAllToken = parent.selectAllToken
        }

        private func applySelection(_ ids: Set<Int64>) {
            guard let collection, let parent, let section = sections.firstIndex(of: .tracks) else { return }
            let wanted = Set(parent.tracks.enumerated().filter { ids.contains($0.element.id) }
                .map { IndexPath(item: $0.offset, section: section) })
            guard wanted != collection.selectionIndexPaths else { return }
            applying = true
            collection.selectionIndexPaths = wanted
            applying = false
        }

        fileprivate func selectionChanged() {
            guard !applying, let collection, let parent, let section = sections.firstIndex(of: .tracks) else { return }
            let ids = Set(collection.selectionIndexPaths.filter { $0.section == section }
                .compactMap { $0.item < parent.tracks.count ? parent.tracks[$0.item].id : nil })
            if ids != parent.selection { parent.selection = ids }
        }

        // MARK: Data

        func numberOfSections(in collectionView: NSCollectionView) -> Int { sections.count }

        func collectionView(_ collectionView: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
            guard let parent, section < sections.count else { return 0 }
            switch sections[section] {
            case .artists: return parent.artists.count
            case .albums: return parent.albums.count
            case .tracks: return parent.tracks.count
            }
        }

        func collectionView(
            _ collectionView: NSCollectionView, itemForRepresentedObjectAt indexPath: IndexPath
        ) -> NSCollectionViewItem {
            let identifier: NSUserInterfaceItemIdentifier = switch sections[indexPath.section] {
            case .artists: ArtistPillItem.identifier
            case .albums: AlbumTile.identifier
            case .tracks: TrackItem.identifier
            }
            let item = collectionView.makeItem(withIdentifier: identifier, for: indexPath)
            configure(item, at: indexPath)
            return item
        }

        private func configure(_ item: NSCollectionViewItem, at path: IndexPath) {
            guard let parent, path.section < sections.count else { return }
            switch sections[path.section] {
            case .artists:
                guard path.item < parent.artists.count else { return }
                (item as? ArtistPillItem)?.show(parent.artists[path.item], coordinator: self)
            case .albums:
                guard path.item < parent.albums.count else { return }
                var context = parent.tileContext
                let environment = environment
                context.menu = { parent.albumMenu($0, environment) }
                (item as? AlbumTile)?.show(parent.albums[path.item], in: context)
            case .tracks:
                guard path.item < parent.tracks.count else { return }
                (item as? TrackItem)?.row.show(parent.tracks[path.item], in: parent.trackContext)
            }
        }

        func collectionView(
            _ collectionView: NSCollectionView, viewForSupplementaryElementOfKind kind: NSCollectionView.SupplementaryElementKind,
            at indexPath: IndexPath
        ) -> NSView {
            let header = collectionView.makeSupplementaryView(
                ofKind: kind, withIdentifier: SectionHeader.identifier, for: indexPath
            ) as? SectionHeader ?? SectionHeader()
            let section = sections[indexPath.section]
            header.title.stringValue = section.title
            header.count.stringValue = parent?.counts == true ? "\(count(of: section))" : ""
            header.needsLayout = true
            return header
        }

        private func count(of section: Section) -> Int {
            guard let parent else { return 0 }
            switch section {
            case .artists: return parent.artists.count
            case .albums: return parent.albums.count
            case .tracks: return parent.tracks.count
            }
        }

        // MARK: Selection

        func collectionView(_ collectionView: NSCollectionView, shouldSelectItemsAt indexPaths: Set<IndexPath>) -> Set<IndexPath> {
            guard parent?.tracksSelect == true else { return [] }
            return indexPaths.filter { $0.section < sections.count && sections[$0.section] == .tracks }
        }

        /// A click on a result track: a tick while picking, a new pick on
        /// ⌘-click, otherwise to where the track lives.
        fileprivate func activate(_ path: IndexPath) {
            guard let parent, let track = track(at: path) else { return }
            if parent.pick?.take(.track(track)) == true { return }
            parent.openTrack(track)
        }

        fileprivate func track(at path: IndexPath) -> Track? {
            guard let parent, path.section < sections.count, sections[path.section] == .tracks,
                  path.item < parent.tracks.count
            else { return nil }
            return parent.tracks[path.item].track
        }

        /// What dragging `playable` carries: the whole pick when it is part of
        /// it, otherwise itself.
        fileprivate func transfers(dragging playable: Playable) -> [PlayableTransfer] {
            if let pick = parent?.pick, pick.isActive, pick.contains(playable.key) {
                return pick.picked.map(PlayableTransfer.init)
            }
            return [PlayableTransfer(playable)]
        }

        fileprivate var picking: Bool { parent?.pick?.isActive == true }
        fileprivate func isPicked(_ key: Playable.Key) -> Bool { parent?.pick?.contains(key) == true }

        func collectionView(_ collectionView: NSCollectionView, didSelectItemsAt indexPaths: Set<IndexPath>) {
            selectionChanged()
        }

        func collectionView(_ collectionView: NSCollectionView, didDeselectItemsAt indexPaths: Set<IndexPath>) {
            selectionChanged()
        }

        // MARK: Actions on tracks

        /// The tracks an action on `path` means: the selection when the track
        /// is in it, otherwise the track alone.
        fileprivate func tracks(at path: IndexPath) -> Set<Int64> {
            guard let parent, let collection, path.section < sections.count, sections[path.section] == .tracks,
                  path.item < parent.tracks.count
            else { return [] }
            if collection.selectionIndexPaths.contains(path) { return parent.selection }
            return [parent.tracks[path.item].id]
        }

        fileprivate func artist(at path: IndexPath) -> Artist? {
            guard let parent, path.section < sections.count, sections[path.section] == .artists,
                  path.item < parent.artists.count
            else { return nil }
            return parent.artists[path.item]
        }

        fileprivate func menu(at path: IndexPath) -> NSMenu? {
            guard let parent else { return nil }
            if let artist = artist(at: path) { return parent.artistMenu(artist, environment) }
            let ids = tracks(at: path)
            return ids.isEmpty ? nil : parent.trackMenu(ids, environment)
        }

        fileprivate func primary(at path: IndexPath?) {
            guard let parent else { return }
            let ids = path.map(tracks(at:)) ?? parent.selection
            if !ids.isEmpty { parent.primaryAction(ids) }
        }

        // MARK: Drag

        func collectionView(_ collectionView: NSCollectionView, canDragItemsAt indexPaths: Set<IndexPath>, with event: NSEvent) -> Bool {
            parent?.tracksSelect == true
                && indexPaths.allSatisfy { $0.section < sections.count && sections[$0.section] == .tracks }
        }

        func collectionView(_ collectionView: NSCollectionView, pasteboardWriterForItemAt indexPath: IndexPath) -> (any NSPasteboardWriting)? {
            guard let parent, sections[indexPath.section] == .tracks, indexPath.item < parent.tracks.count,
                  let track = parent.tracks[indexPath.item].track
            else { return nil }
            return pasteboardItem(PlayableTransfer(.track(track)))
        }

        // MARK: Layout

        fileprivate func size(of path: IndexPath, tile: CGFloat, width: CGFloat) -> NSSize {
            guard let parent, path.section < sections.count else { return .zero }
            switch sections[path.section] {
            case .artists:
                guard path.item < parent.artists.count else { return .zero }
                return ArtistPillItem.size(for: parent.artists[path.item].name)
            case .albums:
                return NSSize(width: tile, height: tile + AlbumTile.captionHeight)
            case .tracks:
                return NSSize(width: width, height: TrackTableRow.artHeight)
            }
        }

        fileprivate func kind(of section: Int) -> Section? {
            section < sections.count ? sections[section] : nil
        }
    }
}

/// What dragging a playable puts on the pasteboard, as every drop target in
/// koan reads it.
@MainActor
func pasteboardItem(_ transfer: PlayableTransfer) -> NSPasteboardItem? {
    guard let data = try? JSONEncoder().encode(transfer) else { return nil }
    let item = NSPasteboardItem()
    item.setData(data, forType: NSPasteboard.PasteboardType(UTType.koanPlayable.identifier))
    item.setString(transfer.name, forType: .string)
    return item
}

// MARK: - Layout

@MainActor
private protocol MixedLayoutSource: AnyObject {
    func size(of path: IndexPath, tile: CGFloat, width: CGFloat) -> NSSize
    func kind(of section: Int) -> MixedCollection.Section?
}

/// Sections stacked down the page: pills wrapping left to right, tiles in
/// as many columns as fit, rows the full width. Laid out by hand — a flow
/// layout spreads a short row of pills across the width.
private final class MixedLayout: NSCollectionViewLayout {
    weak var source: (any MixedLayoutSource)?
    var leading: CGFloat = 0 {
        didSet { invalidateLayout() }
    }

    private let margin: CGFloat = 20
    private let headerHeight: CGFloat = 34
    private var attributes: [IndexPath: NSCollectionViewLayoutAttributes] = [:]
    private var headers: [Int: NSCollectionViewLayoutAttributes] = [:]
    private var height: CGFloat = 0

    override func prepare() {
        super.prepare()
        attributes = [:]
        headers = [:]
        guard let collection = collectionView, let source else { return }
        let left = margin + leading
        let width = max(collection.bounds.width - left - margin, 0)
        let tileSpacing: CGFloat = 16
        let columns = max(1, ((width + tileSpacing) / (140 + tileSpacing)).rounded(.down))
        let tile = min(190, ((width - tileSpacing * (columns - 1)) / columns).rounded(.down))
        var y: CGFloat = 8

        for section in 0..<collection.numberOfSections {
            guard let kind = source.kind(of: section) else { continue }
            let header = NSCollectionViewLayoutAttributes(
                forSupplementaryViewOfKind: NSCollectionView.elementKindSectionHeader,
                with: IndexPath(item: 0, section: section)
            )
            header.frame = CGRect(x: left, y: y, width: width, height: headerHeight)
            headers[section] = header
            y += headerHeight

            let spacing: CGFloat = switch kind {
            case .artists: 8
            case .albums: tileSpacing
            case .tracks: 0
            }
            let lineSpacing: CGFloat = switch kind {
            case .artists: 8
            case .albums: 18
            case .tracks: 0
            }
            var x = left
            var lineHeight: CGFloat = 0
            for item in 0..<collection.numberOfItems(inSection: section) {
                let path = IndexPath(item: item, section: section)
                let size = source.size(of: path, tile: tile, width: width)
                if x > left, x + size.width > left + width {
                    x = left
                    y += lineHeight + lineSpacing
                    lineHeight = 0
                }
                let attribute = NSCollectionViewLayoutAttributes(forItemWith: path)
                attribute.frame = CGRect(x: x, y: y, width: size.width, height: size.height)
                attributes[path] = attribute
                x += size.width + spacing
                lineHeight = max(lineHeight, size.height)
            }
            y += lineHeight + 22
        }
        height = y
    }

    override var collectionViewContentSize: NSSize {
        NSSize(width: collectionView?.bounds.width ?? 0, height: height)
    }

    override func layoutAttributesForElements(in rect: NSRect) -> [NSCollectionViewLayoutAttributes] {
        attributes.values.filter { $0.frame.intersects(rect) } + headers.values.filter { $0.frame.intersects(rect) }
    }

    override func layoutAttributesForItem(at indexPath: IndexPath) -> NSCollectionViewLayoutAttributes? {
        attributes[indexPath]
    }

    override func layoutAttributesForSupplementaryView(
        ofKind elementKind: NSCollectionView.SupplementaryElementKind, at indexPath: IndexPath
    ) -> NSCollectionViewLayoutAttributes? {
        headers[indexPath.section]
    }

    override func shouldInvalidateLayout(forBoundsChange newBounds: NSRect) -> Bool {
        newBounds.width != collectionView?.bounds.width
    }
}

// MARK: - The collection view

/// Hover for every item from one tracking area, the clicks a track row and a
/// pill take for themselves, double-click and Return on tracks, and the menu.
final class MixedCollectionView: NSCollectionView {
    fileprivate weak var owner: MixedCollection.Coordinator?
    private var hovered: NSCollectionViewItem?
    private var linked = false

    private lazy var tracking = NSTrackingArea(
        rect: .zero,
        options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
        owner: self
    )

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if !trackingAreas.contains(tracking) { addTrackingArea(tracking) }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if !trackingAreas.contains(tracking) { addTrackingArea(tracking) }
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        hover(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        hover(at: nil)
    }

    func pointerMayHaveMoved() {
        guard let window, window.isKeyWindow else { return hover(at: nil) }
        let point = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        hover(at: visibleRect.contains(point) ? point : nil)
    }

    private func item(at point: NSPoint) -> (NSCollectionViewItem, IndexPath)? {
        guard let path = indexPathForItem(at: point), let item = item(at: path) else { return nil }
        return (item, path)
    }

    private func hover(at point: NSPoint?) {
        let found = point.flatMap(item(at:))
        if found?.0 !== hovered {
            unhover(hovered)
            hovered = found?.0
        }
        var overLink = false
        if let (item, _) = found, let point {
            let local = item.view.convert(point, from: self)
            switch item {
            case let tile as AlbumTile: overLink = tile.hover(at: local)
            case let track as TrackItem: overLink = track.row.hover(at: local)
            case let pill as ArtistPillItem: pill.hovering = true; overLink = true
            default: break
            }
        }
        if overLink != linked {
            linked = overLink
            (overLink ? NSCursor.pointingHand : NSCursor.arrow).set()
        }
    }

    private func unhover(_ item: NSCollectionViewItem?) {
        switch item {
        case let tile as AlbumTile: tile.hoverEnded()
        case let track as TrackItem: _ = track.row.hover(at: nil)
        case let pill as ArtistPillItem: pill.hovering = false
        default: break
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let (item, path) = item(at: point) else { return super.mouseDown(with: event) }
        if let pill = item as? ArtistPillItem {
            // A pill goes to its artist; it is not something to select.
            pill.pressed(event)
            return
        }
        guard let track = item as? TrackItem else { return super.mouseDown(with: event) }
        let hit = track.row.hit(at: track.view.convert(point, from: self))
        if owner?.parent?.tracksSelect == false {
            if case .button(let action) = hit { return action() }
            resultPressed(event, at: path, hit: hit)
            return
        }
        if case .button(let action) = hit {
            action()
            return
        }
        if event.clickCount == 2 {
            owner?.primary(at: path)
            return
        }
        super.mouseDown(with: event)
        if case .link(let action) = hit, event.clickCount == 1,
           !event.modifierFlags.contains(.command), !event.modifierFlags.contains(.shift) {
            action()
        }
    }

    /// A press on a result track: a click goes where it lives (or ticks it),
    /// a link goes where it names, a drag carries it — or the pick.
    private func resultPressed(_ event: NSEvent, at path: IndexPath, hit: RowHit) {
        guard let window, let owner, let track = owner.track(at: path) else { return }
        let start = event.locationInWindow
        while let next = window.nextEvent(matching: [.leftMouseUp, .leftMouseDragged]) {
            if next.type == .leftMouseUp {
                if case .link(let action) = hit, !owner.picking { action() } else { owner.activate(path) }
                return
            }
            if hypot(next.locationInWindow.x - start.x, next.locationInWindow.y - start.y) > 3 {
                let items = owner.transfers(dragging: .track(track)).compactMap(pasteboardItem).map { item in
                    let dragging = NSDraggingItem(pasteboardWriter: item)
                    let origin = convert(start, from: nil)
                    dragging.setDraggingFrame(NSRect(x: origin.x - 20, y: origin.y - 10, width: 40, height: 20), contents: nil)
                    return dragging
                }
                if !items.isEmpty { beginDraggingSession(with: items, event: event, source: PillDragSource.shared) }
                return
            }
        }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let path = indexPathForItem(at: convert(event.locationInWindow, from: nil)) else { return nil }
        return owner?.menu(at: path)
    }

    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 36, 76: owner?.primary(at: nil)
        case 53: deselectAll(nil); owner?.selectionChanged()
        default: super.keyDown(with: event)
        }
    }
}

// MARK: - Items

/// A section's title.
private final class SectionHeader: NSView, NSCollectionViewElement {
    static let identifier = NSUserInterfaceItemIdentifier("SectionHeader")
    let title = NSTextField(labelWithString: "")
    let count = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        title.font = .systemFont(ofSize: NSFont.preferredFont(forTextStyle: .subheadline).pointSize, weight: .semibold)
        title.textColor = .secondaryLabelColor
        count.font = .monospacedDigitSystemFont(ofSize: NSFont.preferredFont(forTextStyle: .caption1).pointSize, weight: .regular)
        count.textColor = .tertiaryLabelColor
        addSubview(title)
        addSubview(count)
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        let height = ceil(title.intrinsicContentSize.height)
        let width = ceil(title.intrinsicContentSize.width)
        title.frame = CGRect(x: 0, y: bounds.height - height - 6, width: width, height: height)
        let countHeight = ceil(count.intrinsicContentSize.height)
        count.frame = CGRect(x: width + 4, y: title.frame.maxY - countHeight - 1, width: 60, height: countHeight)
    }
}

/// A track row as a collection item, with the selection a table would draw.
private final class TrackItem: NSCollectionViewItem {
    static let identifier = NSUserInterfaceItemIdentifier("TrackItem")
    let row = TrackTableRow()
    private let highlight = CALayer()
    /// The line a table draws between rows.
    private let separator = CALayer()

    override func loadView() {
        let root = NSView()
        root.wantsLayer = true
        highlight.cornerRadius = 6
        highlight.cornerCurve = .continuous
        root.layer?.addSublayer(highlight)
        root.layer?.addSublayer(separator)
        root.addSubview(row)
        view = root
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        highlight.frame = view.bounds
        // At the bottom edge; the view is not flipped.
        separator.frame = CGRect(x: 8, y: 0, width: view.bounds.width - 16, height: 1 / (view.window?.backingScaleFactor ?? 2))
        view.effectiveAppearance.performAsCurrentDrawingAppearance {
            separator.backgroundColor = NSColor.separatorColor.cgColor
        }
        CATransaction.commit()
        row.frame = view.bounds.insetBy(dx: 8, dy: 0)
    }

    override var isSelected: Bool {
        didSet { restyle() }
    }

    private func restyle() {
        let focused = view.window?.firstResponder === collectionView
        view.effectiveAppearance.performAsCurrentDrawingAppearance {
            highlight.backgroundColor = isSelected
                ? (focused ? NSColor.selectedContentBackgroundColor : NSColor.unemphasizedSelectedContentBackgroundColor).cgColor
                : nil
        }
        row.backgroundStyle = isSelected && focused ? .emphasized : .normal
    }
}

/// An artist as a chip, as `ArtistPill` draws it: the mic and the name on a
/// plain capsule, which goes to the artist.
private final class ArtistPillItem: NSCollectionViewItem {
    static let identifier = NSUserInterfaceItemIdentifier("ArtistPillItem")
    private static let font = NSFont.preferredFont(forTextStyle: .callout)

    private let capsule = CALayer()
    private let mic = CALayer()
    private let name = NSTextField(labelWithString: "")
    private var artist: Artist?
    private weak var coordinator: MixedCollection.Coordinator?

    var hovering = false {
        didSet { if hovering != oldValue { restyle() } }
    }

    /// Wide enough for the name, up to the 260 points a pill stops at.
    static func size(for name: String) -> NSSize {
        let text = ceil((name as NSString).size(withAttributes: [.font: font]).width)
        // The label pads its text by a few points of its own.
        let width = min(26 + text + 6 + 11, 260 + 22)
        return NSSize(width: width, height: ceil(font.ascender - font.descender + font.leading) + 12)
    }

    override func loadView() {
        let root = AppearanceView()
        // Made before it is in the window, a pill draws in whatever appearance
        // it has then; drawn again once it knows the window's.
        root.changed = { [weak self] in self?.restyle() }
        root.wantsLayer = true
        root.layer?.addSublayer(capsule)
        mic.contentsGravity = .resizeAspect
        capsule.addSublayer(mic)
        name.font = Self.font
        name.lineBreakMode = .byTruncatingTail
        name.maximumNumberOfLines = 1
        root.addSubview(name)
        view = root
    }

    func show(_ artist: Artist, coordinator: MixedCollection.Coordinator) {
        self.artist = artist
        self.coordinator = coordinator
        name.stringValue = artist.name
        name.toolTip = "Go to \(artist.name)"
        view.setAccessibilityLabel(artist.name)
        view.setAccessibilityRole(.link)
        restyle()
        view.needsLayout = true
    }

    /// The mic, or a tick while the page is picking.
    private var micImage: CGImage? {
        let appearance = view.effectiveAppearance
        guard let coordinator, coordinator.picking, let artist else {
            return Symbol.image("music.mic", size: 9, colours: [.tertiaryLabelColor], appearance: appearance)
        }
        let tint = coordinator.parent?.tileContext.tint ?? .controlAccentColor
        return coordinator.isPicked(Playable.artist(id: artist.id, name: artist.name).key)
            ? Symbol.image("checkmark.circle.fill", size: 10, colours: [.white, tint], appearance: appearance)
            : Symbol.image("circle", size: 10, colours: [.tertiaryLabelColor], appearance: appearance)
    }

    private func restyle() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        view.effectiveAppearance.performAsCurrentDrawingAppearance {
            capsule.backgroundColor = NSColor.quaternaryLabelColor.cgColor
        }
        capsule.opacity = hovering ? 1 : 0.8
        mic.contents = micImage
        CATransaction.commit()
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        let bounds = view.bounds
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        capsule.frame = bounds
        capsule.cornerRadius = bounds.height / 2
        mic.contents = micImage
        mic.frame = CGRect(x: 11, y: (bounds.height - 11) / 2, width: 10, height: 11)
        CATransaction.commit()
        let height = ceil(name.intrinsicContentSize.height)
        name.frame = CGRect(x: 26, y: (bounds.height - height) / 2, width: bounds.width - 26 - 11, height: height)
    }

    /// A press: a click goes to the artist, a drag carries them.
    func pressed(_ event: NSEvent) {
        guard let artist, let window = view.window else { return }
        let start = event.locationInWindow
        while let next = window.nextEvent(matching: [.leftMouseUp, .leftMouseDragged]) {
            let playable = Playable.artist(id: artist.id, name: artist.name)
            if next.type == .leftMouseUp {
                if coordinator?.parent?.pick?.take(playable) == true { return }
                coordinator?.parent?.openArtist(artist.id)
                return
            }
            if hypot(next.locationInWindow.x - start.x, next.locationInWindow.y - start.y) > 3 {
                let transfers = coordinator?.transfers(dragging: playable) ?? [PlayableTransfer(playable)]
                let items = transfers.compactMap(pasteboardItem).map { item in
                    let dragging = NSDraggingItem(pasteboardWriter: item)
                    dragging.setDraggingFrame(view.bounds, contents: nil)
                    return dragging
                }
                if !items.isEmpty { view.beginDraggingSession(with: items, event: event, source: PillDragSource.shared) }
                return
            }
        }
    }
}

/// A view that says when the appearance it draws in changes, or arrives.
private final class AppearanceView: NSView {
    var changed: (() -> Void)?

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        changed?()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        changed?()
    }
}

@MainActor
private final class PillDragSource: NSObject, NSDraggingSource {
    static let shared = PillDragSource()

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }
}
#endif
