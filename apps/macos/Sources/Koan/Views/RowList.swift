import SwiftUI

/// A list of selectable rows that builds only the rows on screen.
///
/// On macOS a SwiftUI `List` is AppKit's table with SwiftUI keeping account of
/// every row, on screen or not, and arriving at a page cost about half a
/// millisecond per row before anything could be drawn: a thousand-track
/// playlist stalled the window for most of a second. This is a lazy stack with
/// the parts of a list koan uses put back — click, ⌘- and ⇧-click, the arrow
/// keys, Return, ⌫, Escape and ⌘A, a menu on the selection and a double-click
/// to play — and its cost follows what is visible.
///
/// On iOS it is the `List` it replaces. UIKit's list builds and sizes cells as
/// they scroll in, so there was nothing to fix there, and it keeps the platform's
/// own touch behaviour.
///
/// Rows mark themselves with `listRow(_:)`, the way a `List`'s rows are tagged.
/// Rows that are not selectable — a header, a grid of tiles — go in without it.
struct RowList<ID: Hashable & Sendable, Content: View, Menu: View>: View {
    @Binding var selection: Set<ID>
    /// Every selectable id in the order shown, for ranges and the arrow keys.
    let order: [ID]
    @ViewBuilder let menu: (Set<ID>) -> Menu
    let primaryAction: (Set<ID>) -> Void
    var onDelete: ((Set<ID>) -> Void)?
    @ViewBuilder let content: () -> Content

    #if os(macOS)
    @Environment(UIState.self) private var ui
    @State private var state = RowSelection()
    @FocusState private var focused: Bool

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    content()
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
            }
            .focusable()
            .focusEffectDisabled()
            .focused($focused)
            .onKeyPress(keys: [.upArrow, .downArrow], phases: [.down, .repeat]) { press in
                let step = press.key == .upArrow ? -1 : 1
                guard let id = state.step(step, extending: press.modifiers.contains(.shift))
                else { return .ignored }
                proxy.scrollTo(id)
                return .handled
            }
            .onKeyPress(.return) {
                guard !selection.isEmpty else { return .ignored }
                primaryAction(selection)
                return .handled
            }
            .onDeleteCommand { if !selection.isEmpty { onDelete?(selection) } }
            .onExitCommand { selection = [] }
        }
        .environment(state)
        .onAppear(perform: connect)
        .onChange(of: order) { _, now in state.order = now.map(AnyHashable.init) }
        .onChange(of: selection) { _, now in state.adopt(Set(now.map(AnyHashable.init))) }
        .onChange(of: focused) { _, now in state.focused = now }
        .onChange(of: ui.selectAllToken) { _, _ in selection = Set(order) }
    }

    /// Hands the selection state what it needs to act for the rows. Once: the
    /// closures reach the page's models, which outlive any one evaluation.
    private func connect() {
        state.order = order.map(AnyHashable.init)
        state.adopt(Set(selection.map(AnyHashable.init)))
        let selection = $selection
        let focused = $focused
        state.write = { ids in selection.wrappedValue = Set(ids.compactMap { $0.base as? ID }) }
        state.focus = { focused.wrappedValue = true }
        state.menu = { ids in AnyView(menu(Set(ids.compactMap { $0.base as? ID }))) }
        state.primary = { ids in primaryAction(Set(ids.compactMap { $0.base as? ID })) }
    }
    #else
    var body: some View {
        List(selection: $selection) {
            content()
        }
        .contextMenu(forSelectionType: ID.self) { ids in
            menu(ids)
        } primaryAction: { ids in
            primaryAction(ids)
        }
    }
    #endif
}

#if os(macOS)
/// The selection a `RowList` keeps, shared with its rows through the
/// environment. Ids are type-erased so a row can find it without knowing the
/// list's id type.
@MainActor
@Observable
final class RowSelection {
    private(set) var selected: Set<AnyHashable> = []
    /// Lit while the list has the keyboard and dimmed otherwise, in a table's
    /// own selection colours: koan's declared accent, which is a neutral, not
    /// the record's tint the controls around it wear.
    var focused = false
    /// Where a ⇧-click or ⇧-arrow range is measured from.
    @ObservationIgnored private var anchor: AnyHashable?
    /// The end of the selection the arrow keys move.
    @ObservationIgnored private var cursor: AnyHashable?
    @ObservationIgnored var order: [AnyHashable] = []
    @ObservationIgnored var write: (Set<AnyHashable>) -> Void = { _ in }
    @ObservationIgnored var focus: () -> Void = {}
    @ObservationIgnored var menu: (Set<AnyHashable>) -> AnyView = { _ in AnyView(EmptyView()) }
    @ObservationIgnored var primary: (Set<AnyHashable>) -> Void = { _ in }

    /// The selection as the page now has it, written from outside: cleared,
    /// select-all, a row removed.
    func adopt(_ ids: Set<AnyHashable>) {
        guard ids != selected else { return }
        selected = ids
        if ids.isEmpty { anchor = nil; cursor = nil }
    }

    func click(_ id: AnyHashable, command: Bool, shift: Bool) {
        focus()
        var next: Set<AnyHashable>
        if shift, let anchor, let range = range(anchor, id) {
            next = command ? selected.union(range) : Set(range)
        } else if command {
            next = selected
            if next.remove(id) == nil { next.insert(id) }
            anchor = id
        } else {
            next = [id]
            anchor = id
        }
        cursor = id
        set(next)
    }

    /// Move the cursor a row up or down, taking the selection with it or, with
    /// ⇧, stretching it from the anchor. Returns the row to bring into view.
    func step(_ by: Int, extending: Bool) -> AnyHashable? {
        guard !order.isEmpty else { return nil }
        let from = cursor.flatMap { order.firstIndex(of: $0) }
        let index = from.map { min(max($0 + by, 0), order.count - 1) } ?? (by > 0 ? 0 : order.count - 1)
        let id = order[index]
        cursor = id
        if extending, let anchor, let range = range(anchor, id) {
            set(Set(range))
        } else {
            anchor = id
            set([id])
        }
        return id
    }

    /// What a menu or a double-click on `id` acts on: the selection when the row
    /// is in it, otherwise the row alone.
    func target(_ id: AnyHashable) -> Set<AnyHashable> {
        selected.contains(id) ? selected : [id]
    }

    private func range(_ from: AnyHashable, _ to: AnyHashable) -> ArraySlice<AnyHashable>? {
        guard let a = order.firstIndex(of: from), let b = order.firstIndex(of: to) else { return nil }
        return order[min(a, b)...max(a, b)]
    }

    private func set(_ ids: Set<AnyHashable>) {
        selected = ids
        write(ids)
    }
}
#endif

extension View {
    /// A selectable row of a `RowList`, the way `tag(_:)` marks a `List`'s.
    func listRow<ID: Hashable>(_ id: ID) -> some View {
        modifier(ListRow(id: id))
    }
}

private struct ListRow<ID: Hashable>: ViewModifier {
    let id: ID

    #if os(macOS)
    @Environment(RowSelection.self) private var selection

    func body(content: Content) -> some View {
        let key = AnyHashable(id)
        let selected = selection.selected.contains(key)
        content
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .background {
                if selected {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(Color(nsColor: selection.focused
                            ? .selectedContentBackgroundColor
                            : .unemphasizedSelectedContentBackgroundColor))
                }
            }
            // How a table tells its rows they are selected, so text and icons
            // that already adapt to it keep doing so.
            .environment(\.backgroundProminence, selected && selection.focused ? .increased : .standard)
            .id(id)
            .simultaneousGesture(TapGesture().onEnded {
                let flags = NSEvent.modifierFlags
                selection.click(key, command: flags.contains(.command), shift: flags.contains(.shift))
            })
            .simultaneousGesture(TapGesture(count: 2).onEnded {
                selection.primary(selection.target(key))
            })
            .contextMenu { selection.menu(selection.target(key)) }
    }
    #else
    func body(content: Content) -> some View {
        content.tag(id)
    }
    #endif
}

/// A section title in a `RowList`. A `List` styles its own; on macOS the rows
/// are a plain stack, so the title brings the style with it.
struct RowListHeader: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    var body: some View {
        #if os(macOS)
        Text(title)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .padding(.top, 12)
            .padding(.bottom, 4)
        #else
        Text(title)
        #endif
    }
}
