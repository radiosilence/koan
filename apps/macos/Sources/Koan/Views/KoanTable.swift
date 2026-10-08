#if os(macOS)
import AppKit
import KoanFFI
import SwiftUI
import UniformTypeIdentifiers

/// What a click on part of a row does.
enum RowHit {
    /// Nothing of its own: the click selects the row.
    case plain
    /// A control — a play mark, a heart. Acts on the press and leaves the
    /// selection alone.
    case button(() -> Void)
    /// A name that goes somewhere. Selects and drags like the rest of the row,
    /// and a click that does not become a drag follows it.
    case link(() -> Void)
}

/// A row of a `KoanTable`, which lays out its own columns from its width.
/// A cell resized by its table is not asked to lay out again by AppKit, so a
/// table narrowed by the lyrics opening left every row's trailing columns
/// past its new edge, prepared rows off screen included.
class TableCell: NSTableCellView {
    override func setFrameSize(_ newSize: NSSize) {
        let resized = newSize.width != frame.width
        super.setFrameSize(newSize)
        if resized { needsLayout = true }
    }
}

/// A row of a `KoanTable`, made of layers and labels.
///
/// Not of AppKit controls: on macOS 26 each is a SwiftUI view graph of its
/// own, and one per row costs what the table exists to avoid. A row makes the
/// few it needs only while it shows them — see `AlbumCollection`.
@MainActor
protocol TableRow: NSTableCellView, HoverableRow {
    associatedtype Item
    associatedtype Context

    static var identifier: NSUserInterfaceItemIdentifier { get }
    static var height: CGFloat { get }

    func show(_ item: Item, in context: Context)
    /// The pointer is at `point` in the row, or has left it. Whether it is
    /// over a link, for the cursor.
    func hover(at point: NSPoint?) -> Bool
    func hit(at point: NSPoint) -> RowHit
}

/// A list on the Mac: an `NSTableView` of `TableRow`s.
///
/// SwiftUI's `List` on macOS sets up every row in the data set, on screen or
/// not, before the page can draw — about a millisecond a row, so a page of a
/// thousand tracks or two thousand artists stalled for most of a second. A
/// table makes the rows on screen and reuses them. Selection, the arrow keys,
/// type-to-select, double-click and multi-row drags are the table's own.
/// iOS keeps `List`, which UIKit already builds lazily.
struct KoanTable<Row: TableRow, ID: Hashable>: NSViewRepresentable {
    let items: [Row.Item]
    let id: (Row.Item) -> ID
    let context: Row.Context
    /// Changes whenever something in `context` that rows draw changes, so
    /// the rows on screen are redrawn then and only then.
    let contextKey: AnyHashable
    @Binding var selection: Set<ID>
    let make: () -> Row
    /// The rows' height, when a list's differs from its row type's.
    var rowHeight = Row.height
    /// Rows that head a run of others — a day in history. They are not
    /// selected, stay at the top while their run scrolls under them, and are
    /// `headingHeight` tall.
    var isHeading: (Row.Item) -> Bool = { _ in false }
    var headingHeight: CGFloat = 28
    /// Each row's height, where a list's rows differ — an album heading in
    /// the queue is taller than its tracks.
    var heightOf: ((Row.Item) -> CGFloat)?
    /// Whether rows that kept their identity changed what they show — a
    /// queue row's status. Visible rows are redrawn when it says so.
    var changed: (([Row.Item], [Row.Item]) -> Bool)?
    /// The context menu for these rows, built with the page's environment.
    var menu: (Set<ID>, EnvironmentValues) -> NSMenu? = { _, _ in nil }
    /// Double-click or Return.
    var primaryAction: (Set<ID>) -> Void = { _ in }
    /// What dragging these rows carries.
    var drag: (Set<ID>) -> [PlayableTransfer] = { _ in [] }
    /// ⌫, where removing rows means something.
    var delete: ((Set<ID>) -> Void)?
    /// ⌘A. A counter, as `UIState.selectAllToken`.
    var selectAllToken = 0
    /// Where the list was scrolled to when the page was last left, and where
    /// to note it now. Not observed — see `LibraryModel.albumsOffset`.
    var offset: CGFloat?
    var noteOffset: (CGFloat) -> Void = { _ in }
    /// Bumped to send the list back to its top.
    var rewinds = 0
    /// A row to select and bring into view, once — arriving from search at
    /// one track of a record.
    var reveal: ID?
    var revealed: () -> Void = {}
    /// Rows dragged within the list and dropped before the row at an index:
    /// a reorder. AppKit draws the line where they would land.
    var move: (([ID], Int) -> Void)?
    /// Playables dropped from elsewhere before the row at an index.
    var accept: (([PlayableTransfer], Int) -> Bool)?
    /// Scroll a row into view: bumped token, the row, and where it should sit.
    var jump: (token: Int, to: ID?, place: JumpPlace) = (0, nil, .centre)
    /// A row to keep in view as it changes, at the place in the viewport the
    /// last one held: the playing track, while the queue follows it.
    var follow: ID?
    /// The person scrolled: the wheel, a trackpad, the scroller or a paging
    /// key. Never the table's own scrolling.
    var userScrolled: () -> Void = {}
    let insets: EdgeInsets

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = KoanTableView()
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("row"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .inset
        table.rowHeight = rowHeight
        table.usesAutomaticRowHeights = false
        table.backgroundColor = .clear
        // The separators a SwiftUI list draws between its rows; none in the
        // theme, whose rows are told apart by rhythm and alignment.
        table.gridStyleMask = KoanTheme.isOn ? [] : .solidHorizontalGridLineMask
        table.gridColor = .koanSeparator
        // A floating group row is drawn on AppKit's own grey band with a rule
        // under it, which no list in the theme has; its headings scroll with
        // their rows instead.
        table.floatsGroupRows = !KoanTheme.isOn
        table.allowsMultipleSelection = true
        table.allowsTypeSelect = true
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.setDraggingSourceOperationMask([.copy, .move], forLocal: true)
        table.setDraggingSourceOperationMask(.copy, forLocal: false)
        var accepted: [NSPasteboard.PasteboardType] = []
        if move != nil { accepted.append(.koanRow) }
        if accept != nil { accepted.append(NSPasteboard.PasteboardType(UTType.koanPlayable.identifier)) }
        if !accepted.isEmpty { table.registerForDraggedTypes(accepted) }

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.contentView.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false

        context.coordinator.attach(table: table, scroll: scroll)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        // The trailing inset is the lyrics, which float over the page as the
        // sidebar does: without it each row's format and length sat under
        // them.
        let content = NSEdgeInsets(top: insets.top, left: insets.leading, bottom: insets.bottom, right: insets.trailing)
        let current = scroll.contentInsets
        if current.top != content.top || current.left != content.left || current.bottom != content.bottom
            || current.right != content.right {
            scroll.contentInsets = content
            // Up under the toolbar, as a SwiftUI scroll view's scroller runs,
            // and clear of the transport. The content's trailing inset already
            // brings it in from under the lyrics.
            scroll.scrollerInsets = NSEdgeInsets(top: 0, left: 0, bottom: insets.bottom, right: 0)
        }
        context.coordinator.update(self, environment: context.environment)
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.detach()
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        private var table: KoanTableView?
        private var scroll: NSScrollView?
        private var parent: KoanTable?
        private var environment = EnvironmentValues()
        private var items: [Row.Item] = []
        private var ids: [ID] = []
        private var index: [ID: Int] = [:]
        private var shownKey: AnyHashable?
        private var selectAllToken: Int?
        private var rewinds: Int?
        private var restored = false
        private var jumpToken: Int?
        private var followed: ID?
        private var liveScroll: (any NSObjectProtocol)?
        /// Set while the table applies a selection it was handed, so the
        /// change is not handed straight back.
        private var applying = false
        private var watching: (any NSObjectProtocol)?

        func attach(table: KoanTableView, scroll: NSScrollView) {
            self.table = table
            self.scroll = scroll
            table.dataSource = self
            table.delegate = self
            table.target = self
            table.doubleAction = #selector(doubleClicked)
            table.owner = self
            scroll.contentView.postsBoundsChangedNotifications = true
            watchLiveScroll(scroll)
            watching = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, let scroll = self.scroll else { return }
                    if self.restored {
                        self.parent?.noteOffset(scroll.contentView.bounds.minY + scroll.contentInsets.top)
                    }
                    self.table?.pointerMayHaveMoved()
                }
            }
        }

        func detach() {
            if let watching { NotificationCenter.default.removeObserver(watching) }
            if let liveScroll { NotificationCenter.default.removeObserver(liveScroll) }
        }

        /// The scroller dragged, or a trackpad's gesture begun. The wheel
        /// and the keys arrive through the table view.
        private func watchLiveScroll(_ scroll: NSScrollView) {
            liveScroll = NotificationCenter.default.addObserver(
                forName: NSScrollView.willStartLiveScrollNotification, object: scroll, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.userScrolled() }
            }
        }

        func userScrolled() {
            parent?.userScrolled()
        }

        func update(_ parent: KoanTable, environment: EnvironmentValues) {
            self.parent = parent
            self.environment = environment
            guard let table else { return }

            let ids = parent.items.map(parent.id)
            if ids != self.ids {
                self.items = parent.items
                self.ids = ids
                index = Dictionary(ids.enumerated().map { ($1, $0) }, uniquingKeysWith: { first, _ in first })
                table.reloadData()
                shownKey = parent.contextKey
            } else if parent.contextKey != shownKey || parent.changed?(self.items, parent.items) == true {
                self.items = parent.items
                shownKey = parent.contextKey
                reshowVisible()
            } else {
                self.items = parent.items
            }
            applySelection(parent.selection)

            if let token = selectAllToken, token != parent.selectAllToken, table.window?.firstResponder === table {
                table.selectAll(nil)
            }
            selectAllToken = parent.selectAllToken
            if let rewinds, rewinds != parent.rewinds { scroll(to: 0) }
            rewinds = parent.rewinds
            if let token = jumpToken, token != parent.jump.token,
               let target = parent.jump.to, let row = index[target] {
                jump(to: row, place: parent.jump.place)
            }
            jumpToken = parent.jump.token
            follow(parent.follow)
            if table.rowHeight != parent.rowHeight {
                table.rowHeight = parent.rowHeight
                table.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<items.count))
            }
            if let wanted = parent.reveal, let row = index[wanted] {
                restored = true
                parent.selection = [wanted]
                table.scrollRowToVisible(row)
                parent.revealed()
            }
        }

        /// Back to where the list was left, once there are rows to scroll:
        /// the first layout with them in it. Not during SwiftUI's update,
        /// where laying the table out is an AppKit exception.
        func laidOut() {
            guard !restored, !ids.isEmpty, let parent else { return }
            restored = true
            if let offset = parent.offset { scroll(to: offset) }
        }

        private func scroll(to offset: CGFloat) {
            guard let scroll else { return }
            scroll.contentView.scroll(to: NSPoint(x: -scroll.contentInsets.left, y: offset - scroll.contentInsets.top))
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        private func jump(to row: Int, place: JumpPlace) {
            guard let table, let scroll else { return }
            let rect = table.rect(ofRow: row)
            let visible = scroll.contentView.bounds.height - scroll.contentInsets.top - scroll.contentInsets.bottom
            let y: CGFloat = switch place {
            case .top: rect.minY
            case .centre: rect.midY - visible / 2
            case .bottom: rect.maxY - visible
            }
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.18
                scroll.contentView.animator().setBoundsOrigin(NSPoint(
                    x: scroll.contentView.bounds.minX,
                    y: max(y, 0) - scroll.contentInsets.top
                ))
            }
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        /// Keep the followed row where the last one sat: scroll by the distance
        /// between them, so a gapless move to the next track slides the list
        /// by a row rather than throwing the track to an edge. A row that was
        /// not on screen, or is gone, is centred instead.
        ///
        /// Recorded only once its row is here and scrolled to: the playing
        /// item and the rows arrive in separate updates, so a track played
        /// from a new queue can be followed before it is listed. Every update
        /// asks again, which catches it when the rows land.
        private func follow(_ target: ID?) {
            guard let target else {
                followed = nil
                return
            }
            guard target != followed, let table, let scroll, let row = index[target] else { return }
            defer { followed = target }
            let visible = scroll.contentView.bounds
            let to = table.rect(ofRow: row)
            let from = followed.flatMap { index[$0] }.map { table.rect(ofRow: $0) }
            let y: CGFloat
            if let from, from.intersects(visible) {
                y = visible.minY + (to.minY - from.minY)
            } else {
                let height = visible.height - scroll.contentInsets.top - scroll.contentInsets.bottom
                y = to.midY - height / 2 - scroll.contentInsets.top
            }
            let top = -scroll.contentInsets.top
            let bottom = max(table.bounds.height - visible.height + scroll.contentInsets.bottom, top)
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.3
                context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                scroll.contentView.animator().setBoundsOrigin(NSPoint(
                    x: visible.minX,
                    y: min(max(y, top), bottom)
                ))
            }
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        private func reshowVisible() {
            guard let table, let parent else { return }
            let rows = table.rows(in: table.visibleRect)
            for row in rows.lowerBound..<rows.upperBound where row < items.count {
                (table.view(atColumn: 0, row: row, makeIfNecessary: false) as? Row)?.show(items[row], in: parent.context)
            }
        }

        private func applySelection(_ selection: Set<ID>) {
            guard let table else { return }
            let wanted = IndexSet(selection.compactMap { index[$0] })
            guard wanted != table.selectedRowIndexes else { return }
            applying = true
            table.selectRowIndexes(wanted, byExtendingSelection: false)
            applying = false
        }

        // MARK: Data and rows

        func numberOfRows(in tableView: NSTableView) -> Int { items.count }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard let parent, row < items.count else { return nil }
            let view = tableView.makeView(withIdentifier: Row.identifier, owner: nil) as? Row ?? {
                let made = parent.make()
                made.identifier = Row.identifier
                return made
            }()
            view.show(items[row], in: parent.context)
            return view
        }

        func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
            guard let parent, row < items.count else { return false }
            return parent.isHeading(items[row])
        }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
            guard let parent, row < items.count else { return tableView.rowHeight }
            if parent.isHeading(items[row]) { return parent.headingHeight }
            return parent.heightOf?(items[row]) ?? parent.rowHeight
        }

        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
            guard let parent, row < items.count else { return false }
            return !parent.isHeading(items[row])
        }

        func tableView(_ tableView: NSTableView, typeSelectStringFor tableColumn: NSTableColumn?, row: Int) -> String? {
            (tableView.view(atColumn: 0, row: row, makeIfNecessary: false) as? Row)?.textField?.stringValue
        }

        func tableViewSelectionDidChange(_ notification: Notification) {
            guard !applying, let table, let parent else { return }
            let picked = Set(table.selectedRowIndexes.compactMap { $0 < ids.count ? ids[$0] : nil })
            if picked != parent.selection { parent.selection = picked }
        }

        // MARK: Actions

        /// The rows an action on `row` means: the selection when the row is
        /// in it, otherwise the row alone — as a SwiftUI list's menu.
        func target(_ row: Int) -> Set<ID> {
            guard let table, row >= 0, row < ids.count else { return [] }
            if table.selectedRowIndexes.contains(row) {
                return Set(table.selectedRowIndexes.compactMap { $0 < ids.count ? ids[$0] : nil })
            }
            return [ids[row]]
        }

        @objc private func doubleClicked() {
            guard let table, table.clickedRow >= 0 else { return }
            parent?.primaryAction(target(table.clickedRow))
        }

        func primary() {
            guard let parent, !parent.selection.isEmpty else { return }
            parent.primaryAction(parent.selection)
        }

        func remove() -> Bool {
            guard let parent, let delete = parent.delete, !parent.selection.isEmpty else { return false }
            delete(parent.selection)
            return true
        }

        func menu(for row: Int) -> NSMenu? {
            guard let parent else { return nil }
            let ids = target(row)
            guard !ids.isEmpty else { return nil }
            return parent.menu(ids, environment)
        }

        // MARK: Drag

        func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> (any NSPasteboardWriting)? {
            guard let parent, row < ids.count else { return nil }
            table?.dragging = true
            let transfer = parent.drag([ids[row]]).first
            guard transfer != nil || parent.move != nil else { return nil }
            let item = transfer.flatMap(pasteboardItem) ?? NSPasteboardItem()
            // Which row, for a drop back into this list to read as a move.
            if parent.move != nil { item.setString(String(row), forType: .koanRow) }
            return item
        }

        // MARK: Drop

        func tableView(
            _ tableView: NSTableView, validateDrop info: any NSDraggingInfo,
            proposedRow row: Int, proposedDropOperation operation: NSTableView.DropOperation
        ) -> NSDragOperation {
            guard let parent else { return [] }
            // Between rows, never onto one: a drop lands before the row.
            if operation == .on { tableView.setDropRow(row, dropOperation: .above) }
            if (info.draggingSource as AnyObject?) === tableView, parent.move != nil,
               info.draggingPasteboard.types?.contains(.koanRow) == true {
                return .move
            }
            return parent.accept == nil ? [] : .copy
        }

        func tableView(
            _ tableView: NSTableView, acceptDrop info: any NSDraggingInfo,
            row: Int, dropOperation: NSTableView.DropOperation
        ) -> Bool {
            guard let parent else { return false }
            let items = info.draggingPasteboard.pasteboardItems ?? []
            if (info.draggingSource as AnyObject?) === tableView, let move = parent.move {
                let rows = items.compactMap { $0.string(forType: .koanRow).flatMap(Int.init) }
                let moving = rows.sorted().compactMap { $0 < ids.count ? ids[$0] : nil }
                if !moving.isEmpty {
                    move(moving, row)
                    return true
                }
            }
            guard let accept = parent.accept else { return false }
            let type = NSPasteboard.PasteboardType(UTType.koanPlayable.identifier)
            let transfers = items.compactMap { $0.data(forType: type) }
                .compactMap { try? JSONDecoder().decode(PlayableTransfer.self, from: $0) }
            return !transfers.isEmpty && accept(transfers, row)
        }
    }
}

/// Where a row asked for should sit once scrolled to.
enum JumpPlace {
    case top, centre, bottom
}

extension NSPasteboard.PasteboardType {
    /// A row of a `KoanTable` being dragged within it.
    static let koanRow = NSPasteboard.PasteboardType("cc.blit.koan.row")
}

/// The table under a `KoanTable`: hover for every row from one tracking area,
/// clicks on a row's controls and links, the menu and the keys a SwiftUI list
/// answered.
final class KoanTableView: NSTableView {
    fileprivate weak var owner: AnyObject?
    fileprivate var dragging = false
    private var hovered: NSTableCellView?
    private var linked = false

    private var coordinator: (any KoanTableActions)? { owner as? any KoanTableActions }

    override func layout() {
        super.layout()
        coordinator?.laidOut()
    }

    private lazy var tracking = NSTrackingArea(
        rect: .zero,
        options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
        owner: self
    )

    /// Ours, alongside the ones the table keeps for itself — which it
    /// rebuilds here, taking any others with them.
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

    override func mouseEntered(with event: NSEvent) {
        super.mouseEntered(with: event)
        hover(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        hover(at: nil)
    }

    /// The rows move under a pointer that stays still while the list scrolls.
    func pointerMayHaveMoved() {
        guard let window, window.isKeyWindow else { return hover(at: nil) }
        let point = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        hover(at: visibleRect.contains(point) ? point : nil)
    }

    private func cell(at point: NSPoint) -> (NSTableCellView, NSPoint)? {
        let row = row(at: point)
        guard row >= 0, let cell = view(atColumn: 0, row: row, makeIfNecessary: false) as? NSTableCellView else {
            return nil
        }
        return (cell, cell.convert(point, from: self))
    }

    private func hover(at point: NSPoint?) {
        let found = point.flatMap(cell(at:))
        if found?.0 !== hovered {
            _ = (hovered as? any HoverableRow)?.hover(at: nil)
            hovered = found?.0
        }
        let overLink = found.map { cell, local in (cell as? any HoverableRow)?.hover(at: local) ?? false } ?? false
        if overLink != linked {
            linked = overLink
            (overLink ? NSCursor.pointingHand : NSCursor.arrow).set()
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let hit = cell(at: point).flatMap { cell, local in (cell as? any HoverableRow)?.hit(at: local) } ?? .plain
        if case .button(let action) = hit {
            action()
            return
        }
        dragging = false
        // The table's own tracking — selection, drags, double-click — runs
        // until the button comes up.
        super.mouseDown(with: event)
        if case .link(let action) = hit, !dragging, event.clickCount == 1,
           !event.modifierFlags.contains(.command), !event.modifierFlags.contains(.shift) {
            action()
        }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let row = row(at: convert(event.locationInWindow, from: nil))
        return coordinator?.menu(for: row)
    }

    override func scrollWheel(with event: NSEvent) {
        coordinator?.userScrolled()
        super.scrollWheel(with: event)
    }

    override func keyDown(with event: NSEvent) {
        // Page Up, Page Down, Home, End: the person moving the list.
        if [116, 121, 115, 119].contains(event.keyCode) { coordinator?.userScrolled() }
        switch event.keyCode {
        case 36, 76: coordinator?.primary()
        case 51, 117: if coordinator?.remove() != true { super.keyDown(with: event) }
        case 53: deselectAll(nil)
        default: super.keyDown(with: event)
        }
    }
}

/// What `KoanTableView` asks of its coordinator, without knowing its row type.
@MainActor
protocol KoanTableActions: AnyObject {
    func menu(for row: Int) -> NSMenu?
    func laidOut()
    func primary()
    func remove() -> Bool
    func userScrolled()
}

extension KoanTable.Coordinator: KoanTableActions {}

/// `TableRow` without its associated types, for the table view to talk to.
@MainActor
protocol HoverableRow {
    func hover(at point: NSPoint?) -> Bool
    func hit(at point: NSPoint) -> RowHit
}

/// The page's safe area — the toolbar, the floating sidebar, the transport —
/// handed to AppKit content that ignores it so it can scroll under them.
struct SafeAreaReader<Content: View>: View {
    @ViewBuilder let content: (EdgeInsets) -> Content
    @State private var insets = EdgeInsets()

    var body: some View {
        content(insets)
            .ignoresSafeArea()
            .background {
                Color.clear.onGeometryChange(for: EdgeInsets.self) { $0.safeAreaInsets } action: { insets = $0 }
            }
    }
}

/// A SwiftUI menu as an `NSMenu`, reading what the page reads.
@MainActor
func hostedMenu(_ menu: some View, environment: EnvironmentValues) -> NSMenu {
    // A hosted menu shows a label's title alone unless asked for its icon,
    // which a SwiftUI context menu shows by itself.
    NSHostingMenu(rootView: menu
        .labelStyle(.titleAndIcon)
        .transformEnvironment(\.self) { $0 = environment })
}
#endif
