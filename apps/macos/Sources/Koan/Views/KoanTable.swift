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
        // The separators a SwiftUI list draws between its rows.
        table.gridStyleMask = .solidHorizontalGridLineMask
        table.gridColor = .separatorColor
        table.allowsMultipleSelection = true
        table.allowsTypeSelect = true
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.setDraggingSourceOperationMask(.copy, forLocal: true)
        table.setDraggingSourceOperationMask(.copy, forLocal: false)

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
        let content = NSEdgeInsets(top: insets.top, left: insets.leading, bottom: insets.bottom, right: 0)
        let current = scroll.contentInsets
        if current.top != content.top || current.left != content.left || current.bottom != content.bottom {
            scroll.contentInsets = content
            // Up under the toolbar, as a SwiftUI scroll view's scroller runs,
            // and clear of the transport.
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
            } else if parent.contextKey != shownKey {
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
            return parent.isHeading(items[row]) ? parent.headingHeight : parent.rowHeight
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
            guard let transfer = parent.drag([ids[row]]).first,
                  let data = try? JSONEncoder().encode(transfer)
            else { return nil }
            let item = NSPasteboardItem()
            item.setData(data, forType: NSPasteboard.PasteboardType(UTType.koanPlayable.identifier))
            item.setString(transfer.name, forType: .string)
            return item
        }
    }
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

    override func keyDown(with event: NSEvent) {
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
    NSHostingMenu(rootView: menu.transformEnvironment(\.self) { $0 = environment })
}
#endif
